use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::{Debug, Formatter};
use std::fs::File;
use std::io::BufWriter;
use std::str::FromStr;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gethostname::gethostname;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::{error, warn};

use matrix_sdk::{
    config::{RequestConfig, SyncSettings},
    encryption::verification::{SasVerification, Verification},
    event_handler::Ctx,
    reqwest,
    room::{Invited, Messages, MessagesOptions, Room as MatrixRoom, RoomMember},
    ruma::{
        api::client::{
            filter::{FilterDefinition, LazyLoadOptions, RoomEventFilter, RoomFilter},
            room::create_room::v3::{CreationContent, Request as CreateRoomRequest, RoomPreset},
            room::Visibility,
            space::get_hierarchy::v1::Request as SpaceHierarchyRequest,
        },
        assign,
        events::{
            key::verification::{
                done::{OriginalSyncKeyVerificationDoneEvent, ToDeviceKeyVerificationDoneEvent},
                key::{OriginalSyncKeyVerificationKeyEvent, ToDeviceKeyVerificationKeyEvent},
                request::ToDeviceKeyVerificationRequestEvent,
                start::{OriginalSyncKeyVerificationStartEvent, ToDeviceKeyVerificationStartEvent},
                VerificationMethod,
            },
            presence::PresenceEvent,
            reaction::ReactionEventContent,
            room::{
                encryption::RoomEncryptionEventContent,
                message::{MessageType, RoomMessageEventContent},
                name::RoomNameEventContent,
                redaction::{OriginalSyncRoomRedactionEvent, SyncRoomRedactionEvent},
            },
            tag::Tags,
            typing::SyncTypingEvent,
            AnyInitialStateEvent,
            AnyMessageLikeEvent,
            AnyTimelineEvent,
            EmptyStateKey,
            InitialStateEvent,
            SyncMessageLikeEvent,
            SyncStateEvent,
        },
        room::RoomType,
        serde::Raw,
        EventEncryptionAlgorithm,
        OwnedRoomId,
        OwnedRoomOrAliasId,
        OwnedUserId,
        RoomId,
        RoomVersionId,
    },
    Client,
    DisplayName,
    Session,
};

use modalkit::editing::action::{EditInfo, InfoMessage, UIError};

#[cfg(feature = "sixel")]
use crate::message::sixel;

use crate::{
    base::{
        AsyncProgramStore,
        ChatStore,
        CreateRoomFlags,
        CreateRoomType,
        EventLocation,
        IambError,
        IambResult,
        Receipts,
        RoomFetchStatus,
        VerifyAction,
    },
    message::MessageFetchResult,
    ApplicationSettings,
};

const IAMB_DEVICE_NAME: &str = "iamb";
const IAMB_USER_AGENT: &str = "iamb";
const MIN_MSG_LOAD: u32 = 50;

fn initial_devname() -> String {
    format!("{} on {}", IAMB_DEVICE_NAME, gethostname().to_string_lossy())
}

pub async fn create_room(
    client: &Client,
    room_alias_name: Option<&str>,
    rt: CreateRoomType,
    flags: CreateRoomFlags,
) -> IambResult<OwnedRoomId> {
    let mut creation_content = None;
    let mut initial_state = vec![];
    let mut is_direct = false;
    let mut preset = None;
    let mut invite = vec![];

    let visibility = if flags.contains(CreateRoomFlags::PUBLIC) {
        Visibility::Public
    } else {
        Visibility::Private
    };

    match rt {
        CreateRoomType::Direct(user) => {
            invite.push(user);
            is_direct = true;
            preset = Some(RoomPreset::TrustedPrivateChat);
        },
        CreateRoomType::Space => {
            let mut cc = CreationContent::new();
            cc.room_type = Some(RoomType::Space);

            let raw_cc = Raw::new(&cc).map_err(IambError::from)?;
            creation_content = Some(raw_cc);
        },
        CreateRoomType::Room => {},
    }

    // Set up encryption.
    if flags.contains(CreateRoomFlags::ENCRYPTED) {
        // XXX: Once matrix-sdk uses ruma 0.8, then this can skip the cast.
        let algo = EventEncryptionAlgorithm::MegolmV1AesSha2;
        let content = RoomEncryptionEventContent::new(algo);
        let encr = InitialStateEvent { content, state_key: EmptyStateKey };
        let encr_raw = Raw::new(&encr).map_err(IambError::from)?;
        let encr_raw = encr_raw.cast::<AnyInitialStateEvent>();
        initial_state.push(encr_raw);
    }

    let request = assign!(CreateRoomRequest::new(), {
        room_alias_name,
        creation_content,
        initial_state: initial_state.as_slice(),
        invite: invite.as_slice(),
        is_direct,
        visibility,
        preset,
    });

    let resp = client.create_room(request).await.map_err(IambError::from)?;

    if is_direct {
        if let Some(room) = client.get_room(&resp.room_id) {
            room.set_is_direct(true).await.map_err(IambError::from)?;
        } else {
            error!(
                room_id = resp.room_id.as_str(),
                "Couldn't set is_direct for new direct message room"
            );
        }
    }

    return Ok(resp.room_id);
}

async fn load_plan(store: &AsyncProgramStore) -> HashMap<OwnedRoomId, Option<String>> {
    let mut locked = store.lock().await;
    let ChatStore { need_load, rooms, .. } = &mut locked.application;
    let mut plan = HashMap::new();

    for room_id in std::mem::take(need_load).into_iter() {
        let info = rooms.get_or_default(room_id.clone());

        if info.recently_fetched() || info.fetching {
            need_load.insert(room_id);
            continue;
        } else {
            info.fetch_last = Instant::now().into();
            info.fetching = true;
        }

        let fetch_id = match &info.fetch_id {
            RoomFetchStatus::Done => continue,
            RoomFetchStatus::HaveMore(fetch_id) => Some(fetch_id.clone()),
            RoomFetchStatus::NotStarted => None,
        };

        plan.insert(room_id, fetch_id);
    }

    return plan;
}

async fn load_older_one(
    client: Client,
    room_id: &RoomId,
    fetch_id: Option<String>,
    limit: u32,
) -> MessageFetchResult {
    if let Some(room) = client.get_room(room_id) {
        let mut opts = match &fetch_id {
            Some(id) => MessagesOptions::backward().from(id.as_str()),
            None => MessagesOptions::backward(),
        };
        opts.limit = limit.into();

        let Messages { end, chunk, .. } = room.messages(opts).await.map_err(IambError::from)?;

        let msgs = chunk.into_iter().filter_map(|ev| {
            match ev.event.deserialize() {
                Ok(AnyTimelineEvent::MessageLike(msg)) => Some(msg),
                Ok(AnyTimelineEvent::State(_)) => None,
                Err(_) => None,
            }
        });

        Ok((end, msgs.collect()))
    } else {
        Err(IambError::UnknownRoom(room_id.to_owned()).into())
    }
}

async fn load_insert(room_id: OwnedRoomId, res: MessageFetchResult, store: AsyncProgramStore) {
    let mut locked = store.lock().await;
    let ChatStore { need_load, presences, rooms, .. } = &mut locked.application;
    let info = rooms.get_or_default(room_id.clone());
    info.fetching = false;

    match res {
        Ok((fetch_id, msgs)) => {
            for msg in msgs.into_iter() {
                let sender = msg.sender().to_owned();
                let _ = presences.get_or_default(sender);

                match msg {
                    AnyMessageLikeEvent::RoomEncrypted(msg) => {
                        info.insert_encrypted(msg);
                    },
                    AnyMessageLikeEvent::RoomMessage(msg) => {
                        info.insert(msg);
                    },
                    AnyMessageLikeEvent::Reaction(ev) => {
                        info.insert_reaction(ev);
                    },
                    _ => continue,
                }
            }

            info.fetch_id = fetch_id.map_or(RoomFetchStatus::Done, RoomFetchStatus::HaveMore);
        },
        Err(e) => {
            warn!(room_id = room_id.as_str(), err = e.to_string(), "Failed to load older messages");

            // Wait and try again.
            need_load.insert(room_id);
        },
    }
}

async fn load_older(client: &Client, store: &AsyncProgramStore) {
    let limit = MIN_MSG_LOAD;
    let plan = load_plan(store).await;

    // Fetch each room separately, so they don't block each other.
    for (room_id, fetch_id) in plan.into_iter() {
        let client = client.clone();
        let store = store.clone();

        tokio::spawn(async move {
            let res = load_older_one(client, room_id.as_ref(), fetch_id, limit).await;
            load_insert(room_id, res, store).await;
        });
    }
}

#[derive(Debug)]
pub enum LoginStyle {
    SessionRestore(Session),
    Password(String),
}

pub struct ClientResponse<T>(Receiver<T>);
pub struct ClientReply<T>(SyncSender<T>);

impl<T> ClientResponse<T> {
    fn recv(self) -> T {
        self.0.recv().expect("failed to receive response from client thread")
    }
}

impl<T> ClientReply<T> {
    fn send(self, t: T) {
        self.0.send(t).unwrap();
    }
}

fn oneshot<T>() -> (ClientReply<T>, ClientResponse<T>) {
    let (tx, rx) = sync_channel(1);
    let reply = ClientReply(tx);
    let response = ClientResponse(rx);

    return (reply, response);
}

async fn update_receipts(client: &Client) -> Vec<(OwnedRoomId, Receipts)> {
    let mut rooms = vec![];

    for room in client.joined_rooms() {
        if let Ok(users) = room.active_members_no_sync().await {
            let mut receipts = Receipts::new();

            for member in users {
                let res = room.user_read_receipt(member.user_id()).await;

                if let Ok(Some((event_id, _))) = res {
                    let user_id = member.user_id().to_owned();
                    receipts.entry(event_id).or_default().push(user_id);
                }
            }

            rooms.push((room.room_id().to_owned(), receipts));
        }
    }

    return rooms;
}

pub type FetchedRoom = (MatrixRoom, DisplayName, Option<Tags>);

pub enum WorkerTask {
    ActiveRooms(ClientReply<Vec<FetchedRoom>>),
    DirectMessages(ClientReply<Vec<FetchedRoom>>),
    Init(AsyncProgramStore, ClientReply<()>),
    Login(LoginStyle, ClientReply<IambResult<EditInfo>>),
    GetInviter(Invited, ClientReply<IambResult<Option<RoomMember>>>),
    GetRoom(OwnedRoomId, ClientReply<IambResult<FetchedRoom>>),
    JoinRoom(String, ClientReply<IambResult<OwnedRoomId>>),
    Members(OwnedRoomId, ClientReply<IambResult<Vec<RoomMember>>>),
    SpaceMembers(OwnedRoomId, ClientReply<IambResult<Vec<OwnedRoomId>>>),
    Spaces(ClientReply<Vec<(MatrixRoom, DisplayName)>>),
    TypingNotice(OwnedRoomId),
    Verify(VerifyAction, SasVerification, ClientReply<IambResult<EditInfo>>),
    VerifyRequest(OwnedUserId, ClientReply<IambResult<EditInfo>>),
}

impl Debug for WorkerTask {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            WorkerTask::ActiveRooms(_) => {
                f.debug_tuple("WorkerTask::ActiveRooms").field(&format_args!("_")).finish()
            },
            WorkerTask::DirectMessages(_) => {
                f.debug_tuple("WorkerTask::DirectMessages")
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::Init(_, _) => {
                f.debug_tuple("WorkerTask::Init")
                    .field(&format_args!("_"))
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::Login(style, _) => {
                f.debug_tuple("WorkerTask::Login")
                    .field(style)
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::GetInviter(invite, _) => {
                f.debug_tuple("WorkerTask::GetInviter").field(invite).finish()
            },
            WorkerTask::GetRoom(room_id, _) => {
                f.debug_tuple("WorkerTask::GetRoom")
                    .field(room_id)
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::JoinRoom(s, _) => {
                f.debug_tuple("WorkerTask::JoinRoom")
                    .field(s)
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::Members(room_id, _) => {
                f.debug_tuple("WorkerTask::Members")
                    .field(room_id)
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::SpaceMembers(room_id, _) => {
                f.debug_tuple("WorkerTask::SpaceMembers")
                    .field(room_id)
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::Spaces(_) => {
                f.debug_tuple("WorkerTask::Spaces").field(&format_args!("_")).finish()
            },
            WorkerTask::TypingNotice(room_id) => {
                f.debug_tuple("WorkerTask::TypingNotice").field(room_id).finish()
            },
            WorkerTask::Verify(act, sasv1, _) => {
                f.debug_tuple("WorkerTask::Verify")
                    .field(act)
                    .field(sasv1)
                    .field(&format_args!("_"))
                    .finish()
            },
            WorkerTask::VerifyRequest(user_id, _) => {
                f.debug_tuple("WorkerTask::VerifyRequest")
                    .field(user_id)
                    .field(&format_args!("_"))
                    .finish()
            },
        }
    }
}

#[derive(Clone)]
pub struct Requester {
    pub client: Client,
    pub tx: UnboundedSender<WorkerTask>,
}

impl Requester {
    pub fn init(&self, store: AsyncProgramStore) {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::Init(store, reply)).unwrap();

        return response.recv();
    }

    pub fn login(&self, style: LoginStyle) -> IambResult<EditInfo> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::Login(style, reply)).unwrap();

        return response.recv();
    }

    pub fn direct_messages(&self) -> Vec<FetchedRoom> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::DirectMessages(reply)).unwrap();

        return response.recv();
    }

    pub fn get_inviter(&self, invite: Invited) -> IambResult<Option<RoomMember>> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::GetInviter(invite, reply)).unwrap();

        return response.recv();
    }

    pub fn get_room(&self, room_id: OwnedRoomId) -> IambResult<FetchedRoom> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::GetRoom(room_id, reply)).unwrap();

        return response.recv();
    }

    pub fn join_room(&self, name: String) -> IambResult<OwnedRoomId> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::JoinRoom(name, reply)).unwrap();

        return response.recv();
    }

    pub fn active_rooms(&self) -> Vec<FetchedRoom> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::ActiveRooms(reply)).unwrap();

        return response.recv();
    }

    pub fn members(&self, room_id: OwnedRoomId) -> IambResult<Vec<RoomMember>> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::Members(room_id, reply)).unwrap();

        return response.recv();
    }

    pub fn space_members(&self, space: OwnedRoomId) -> IambResult<Vec<OwnedRoomId>> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::SpaceMembers(space, reply)).unwrap();

        return response.recv();
    }

    pub fn spaces(&self) -> Vec<(MatrixRoom, DisplayName)> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::Spaces(reply)).unwrap();

        return response.recv();
    }

    pub fn typing_notice(&self, room_id: OwnedRoomId) {
        self.tx.send(WorkerTask::TypingNotice(room_id)).unwrap();
    }

    pub fn verify(&self, act: VerifyAction, sas: SasVerification) -> IambResult<EditInfo> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::Verify(act, sas, reply)).unwrap();

        return response.recv();
    }

    pub fn verify_request(&self, user_id: OwnedUserId) -> IambResult<EditInfo> {
        let (reply, response) = oneshot();

        self.tx.send(WorkerTask::VerifyRequest(user_id, reply)).unwrap();

        return response.recv();
    }
}

pub struct ClientWorker {
    initialized: bool,
    settings: ApplicationSettings,
    client: Client,
    load_handle: Option<JoinHandle<()>>,
    rcpt_handle: Option<JoinHandle<()>>,
    sync_handle: Option<JoinHandle<()>>,
}

impl ClientWorker {
    pub async fn spawn(settings: ApplicationSettings) -> Requester {
        let (tx, rx) = unbounded_channel();
        let account = &settings.profile;

        let req_timeout = Duration::from_secs(settings.tunables.request_timeout);

        // Set up the HTTP client.
        let http = reqwest::Client::builder()
            .user_agent(IAMB_USER_AGENT)
            .timeout(req_timeout)
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(10)
            .tcp_keepalive(Duration::from_secs(10))
            .build()
            .unwrap();

        let req_config = RequestConfig::new().timeout(req_timeout).retry_timeout(req_timeout);

        // Set up the Matrix client for the selected profile.
        let client = Client::builder()
            .http_client(Arc::new(http))
            .homeserver_url(account.url.clone())
            .sled_store(settings.matrix_dir.as_path(), None)
            .expect("Failed to setup up sled store for Matrix SDK")
            .request_config(req_config)
            .build()
            .await
            .expect("Failed to instantiate Matrix client");

        let mut worker = ClientWorker {
            initialized: false,
            settings,
            client: client.clone(),
            load_handle: None,
            rcpt_handle: None,
            sync_handle: None,
        };

        tokio::spawn(async move {
            worker.work(rx).await;
        });

        return Requester { client, tx };
    }

    async fn work(&mut self, mut rx: UnboundedReceiver<WorkerTask>) {
        loop {
            let t = rx.recv().await;

            match t {
                Some(task) => self.run(task).await,
                None => {
                    break;
                },
            }
        }

        if let Some(handle) = self.sync_handle.take() {
            handle.abort();
        }

        if let Some(handle) = self.rcpt_handle.take() {
            handle.abort();
        }
    }

    async fn run(&mut self, task: WorkerTask) {
        match task {
            WorkerTask::DirectMessages(reply) => {
                assert!(self.initialized);
                reply.send(self.direct_messages().await);
            },
            WorkerTask::Init(store, reply) => {
                assert_eq!(self.initialized, false);
                self.init(store).await;
                reply.send(());
            },
            WorkerTask::JoinRoom(room_id, reply) => {
                assert!(self.initialized);
                reply.send(self.join_room(room_id).await);
            },
            WorkerTask::GetInviter(invited, reply) => {
                assert!(self.initialized);
                reply.send(self.get_inviter(invited).await);
            },
            WorkerTask::GetRoom(room_id, reply) => {
                assert!(self.initialized);
                reply.send(self.get_room(room_id).await);
            },
            WorkerTask::ActiveRooms(reply) => {
                assert!(self.initialized);
                reply.send(self.active_rooms().await);
            },
            WorkerTask::Login(style, reply) => {
                assert!(self.initialized);
                reply.send(self.login_and_sync(style).await);
            },
            WorkerTask::Members(room_id, reply) => {
                assert!(self.initialized);
                reply.send(self.members(room_id).await);
            },
            WorkerTask::SpaceMembers(space, reply) => {
                assert!(self.initialized);
                reply.send(self.space_members(space).await);
            },
            WorkerTask::Spaces(reply) => {
                assert!(self.initialized);
                reply.send(self.spaces().await);
            },
            WorkerTask::TypingNotice(room_id) => {
                assert!(self.initialized);
                self.typing_notice(room_id).await;
            },
            WorkerTask::Verify(act, sas, reply) => {
                assert!(self.initialized);
                reply.send(self.verify(act, sas).await);
            },
            WorkerTask::VerifyRequest(user_id, reply) => {
                assert!(self.initialized);
                reply.send(self.verify_request(user_id).await);
            },
        }
    }

    async fn init(&mut self, store: AsyncProgramStore) {
        self.client.add_event_handler_context(store.clone());

        let _ = self.client.add_event_handler(
            |ev: SyncTypingEvent, room: MatrixRoom, store: Ctx<AsyncProgramStore>| {
                async move {
                    let room_id = room.room_id().to_owned();
                    let mut locked = store.lock().await;

                    let users = ev
                        .content
                        .user_ids
                        .into_iter()
                        .filter(|u| u != &locked.application.settings.profile.user_id)
                        .collect();

                    locked.application.get_room_info(room_id).set_typing(users);
                }
            },
        );

        let _ =
            self.client
                .add_event_handler(|ev: PresenceEvent, store: Ctx<AsyncProgramStore>| {
                    async move {
                        let mut locked = store.lock().await;
                        locked.application.presences.insert(ev.sender, ev.content.presence);
                    }
                });

        let _ = self.client.add_event_handler(
            |ev: SyncStateEvent<RoomNameEventContent>,
             room: MatrixRoom,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    if let SyncStateEvent::Original(ev) = ev {
                        if let Some(room_name) = ev.content.name {
                            let room_id = room.room_id().to_owned();
                            let room_name = Some(room_name.to_string());
                            let mut locked = store.lock().await;
                            let mut info = locked.application.rooms.get_or_default(room_id.clone());
                            info.name = room_name;
                        }
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: SyncMessageLikeEvent<RoomMessageEventContent>,
             room: MatrixRoom,
             client: Client,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let room_id = room.room_id();
                    let room_name = room.display_name().await.ok();
                    let room_name = room_name.as_ref().map(ToString::to_string);

                    if let Some(msg) = ev.as_original() {
                        if let MessageType::VerificationRequest(_) = msg.content.msgtype {
                            if let Some(request) = client
                                .encryption()
                                .get_verification_request(ev.sender(), ev.event_id())
                                .await
                            {
                                request.accept().await.expect("Failed to accept request");
                            }
                        }
                    }

                    let mut locked = store.lock().await;

                    let sender = ev.sender().to_owned();
                    let _ = locked.application.presences.get_or_default(sender);

                    let mut info = locked.application.get_room_info(room_id.to_owned());
                    info.name = room_name.clone();
                    let full_ev = ev.clone().into_full_event(room_id.to_owned());

                    info.insert(full_ev);

                    #[cfg(feature = "sixel")]
                    if let Some((source, event_id)) = sixel::get_attachment_source(&ev) {
                        match sixel::download(
                            &locked.application.worker.client.media(),
                            source,
                            event_id,
                            &locked.application.settings,
                        )
                        .await
                        .and_then(sixel::load_file_from_path)
                        .and_then(|image| {
                            let info = locked.application.get_room_info(room_id.to_owned());
                            let msg = info.get_event_mut(ev.event_id());
                            return msg
                                .ok_or(UIError::Failure(
                                    "Could not retrieve message for image preview".to_string(),
                                ))
                                .map(|msg| (image, msg));
                        }) {
                            Ok((image, msg)) => msg.image = Some(image),
                            // XXX: should display the download error to user.
                            Err(err) => eprintln!("{err}"),
                        }
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: SyncMessageLikeEvent<ReactionEventContent>,
             room: MatrixRoom,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let room_id = room.room_id();

                    let mut locked = store.lock().await;

                    let sender = ev.sender().to_owned();
                    let _ = locked.application.presences.get_or_default(sender);

                    let info = locked.application.get_room_info(room_id.to_owned());
                    info.insert_reaction(ev.into_full_event(room_id.to_owned()));
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: OriginalSyncRoomRedactionEvent,
             room: MatrixRoom,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let room_id = room.room_id();
                    let room_info = room.clone_info();
                    let room_version = room_info.room_version().unwrap_or(&RoomVersionId::V1);

                    let mut locked = store.lock().await;
                    let info = locked.application.get_room_info(room_id.to_owned());

                    match info.keys.get(&ev.redacts) {
                        None => return,
                        Some(EventLocation::Message(key)) => {
                            if let Some(msg) = info.messages.get_mut(key) {
                                let ev = SyncRoomRedactionEvent::Original(ev);
                                msg.redact(ev, room_version);
                            }
                        },
                        Some(EventLocation::Reaction(event_id)) => {
                            if let Some(reactions) = info.reactions.get_mut(event_id) {
                                reactions.remove(&ev.redacts);
                            }

                            info.keys.remove(&ev.redacts);
                        },
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: OriginalSyncKeyVerificationStartEvent,
             client: Client,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let tx_id = ev.content.relates_to.event_id.as_ref();

                    if let Some(Verification::SasV1(sas)) =
                        client.encryption().get_verification(&ev.sender, tx_id).await
                    {
                        sas.accept().await.unwrap();

                        store.lock().await.application.insert_sas(sas)
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: OriginalSyncKeyVerificationKeyEvent,
             client: Client,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let tx_id = ev.content.relates_to.event_id.as_ref();

                    if let Some(Verification::SasV1(sas)) =
                        client.encryption().get_verification(&ev.sender, tx_id).await
                    {
                        store.lock().await.application.insert_sas(sas);
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: OriginalSyncKeyVerificationDoneEvent,
             client: Client,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let tx_id = ev.content.relates_to.event_id.as_ref();

                    if let Some(Verification::SasV1(sas)) =
                        client.encryption().get_verification(&ev.sender, tx_id).await
                    {
                        store.lock().await.application.insert_sas(sas);
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: ToDeviceKeyVerificationRequestEvent, client: Client| {
                async move {
                    let request = client
                        .encryption()
                        .get_verification_request(&ev.sender, &ev.content.transaction_id)
                        .await
                        .unwrap();

                    request.accept().await.unwrap();
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: ToDeviceKeyVerificationStartEvent,
             client: Client,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let tx_id = ev.content.transaction_id;

                    if let Some(Verification::SasV1(sas)) =
                        client.encryption().get_verification(&ev.sender, tx_id.as_ref()).await
                    {
                        sas.accept().await.unwrap();

                        store.lock().await.application.insert_sas(sas);
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: ToDeviceKeyVerificationKeyEvent, client: Client, store: Ctx<AsyncProgramStore>| {
                async move {
                    let tx_id = ev.content.transaction_id;

                    if let Some(Verification::SasV1(sas)) =
                        client.encryption().get_verification(&ev.sender, tx_id.as_ref()).await
                    {
                        store.lock().await.application.insert_sas(sas);
                    }
                }
            },
        );

        let _ = self.client.add_event_handler(
            |ev: ToDeviceKeyVerificationDoneEvent,
             client: Client,
             store: Ctx<AsyncProgramStore>| {
                async move {
                    let tx_id = ev.content.transaction_id;

                    if let Some(Verification::SasV1(sas)) =
                        client.encryption().get_verification(&ev.sender, tx_id.as_ref()).await
                    {
                        store.lock().await.application.insert_sas(sas);
                    }
                }
            },
        );

        self.rcpt_handle = tokio::spawn({
            let store = store.clone();
            let client = self.client.clone();

            async move {
                // Update the displayed read receipts every 5 seconds.
                let mut interval = tokio::time::interval(Duration::from_secs(5));

                loop {
                    interval.tick().await;

                    let receipts = update_receipts(&client).await;
                    store.lock().await.application.set_receipts(receipts).await;
                }
            }
        })
        .into();

        self.load_handle = tokio::spawn({
            let client = self.client.clone();

            async move {
                // Load older messages every 2 seconds.
                let mut interval = tokio::time::interval(Duration::from_secs(2));
                loop {
                    interval.tick().await;
                    load_older(&client, &store).await;
                }
            }
        })
        .into();

        self.initialized = true;
    }

    async fn login_and_sync(&mut self, style: LoginStyle) -> IambResult<EditInfo> {
        let client = self.client.clone();

        match style {
            LoginStyle::SessionRestore(session) => {
                client.restore_login(session).await.map_err(IambError::from)?;
            },
            LoginStyle::Password(password) => {
                let resp = client
                    .login_username(&self.settings.profile.user_id, &password)
                    .initial_device_display_name(initial_devname().as_str())
                    .send()
                    .await
                    .map_err(IambError::from)?;
                let file = File::create(self.settings.session_json.as_path())?;
                let writer = BufWriter::new(file);
                let session = Session::from(resp);
                serde_json::to_writer(writer, &session).map_err(IambError::from)?;
            },
        }

        let handle = tokio::spawn(async move {
            loop {
                let settings = SyncSettings::default();

                let _ = client.sync(settings).await;
            }
        });

        self.sync_handle = Some(handle);

        // Perform an initial, lazily-loaded sync.
        let mut room = RoomEventFilter::default();
        room.lazy_load_options = LazyLoadOptions::Enabled { include_redundant_members: false };

        let mut room_ev = RoomFilter::default();
        room_ev.state = room;

        let mut filter = FilterDefinition::default();
        filter.room = room_ev;

        let settings = SyncSettings::new().filter(filter.into());

        self.client.sync_once(settings).await.map_err(IambError::from)?;

        Ok(Some(InfoMessage::from("Successfully logged in!")))
    }

    async fn direct_message(&mut self, user: OwnedUserId) -> IambResult<FetchedRoom> {
        for (room, name, tags) in self.direct_messages().await {
            if room.get_member(user.as_ref()).await.map_err(IambError::from)?.is_some() {
                return Ok((room, name, tags));
            }
        }

        let rt = CreateRoomType::Direct(user.clone());
        let flags = CreateRoomFlags::ENCRYPTED;

        match create_room(&self.client, None, rt, flags).await {
            Ok(room_id) => self.get_room(room_id).await,
            Err(e) => {
                error!(
                    user_id = user.as_str(),
                    err = e.to_string(),
                    "Failed to create direct message room"
                );

                let msg = format!("Could not open a room with {user}");
                let err = UIError::Failure(msg);

                Err(err)
            },
        }
    }

    async fn get_inviter(&mut self, invited: Invited) -> IambResult<Option<RoomMember>> {
        let details = invited.invite_details().await.map_err(IambError::from)?;

        Ok(details.inviter)
    }

    async fn get_room(&mut self, room_id: OwnedRoomId) -> IambResult<FetchedRoom> {
        if let Some(room) = self.client.get_room(&room_id) {
            let name = room.display_name().await.map_err(IambError::from)?;
            let tags = room.tags().await.map_err(IambError::from)?;

            Ok((room, name, tags))
        } else {
            Err(IambError::UnknownRoom(room_id).into())
        }
    }

    async fn join_room(&mut self, name: String) -> IambResult<OwnedRoomId> {
        if let Ok(alias_id) = OwnedRoomOrAliasId::from_str(name.as_str()) {
            match self.client.join_room_by_id_or_alias(&alias_id, &[]).await {
                Ok(resp) => Ok(resp.room_id),
                Err(e) => {
                    let msg = e.to_string();
                    let err = UIError::Failure(msg);

                    return Err(err);
                },
            }
        } else if let Ok(user) = OwnedUserId::try_from(name.as_str()) {
            let room = self.direct_message(user).await?.0;

            return Ok(room.room_id().to_owned());
        } else {
            let msg = format!("{:?} is not a valid room or user name", name.as_str());
            let err = UIError::Failure(msg);

            return Err(err);
        }
    }

    async fn direct_messages(&self) -> Vec<FetchedRoom> {
        let mut rooms = vec![];

        for room in self.client.invited_rooms().into_iter() {
            if !room.is_direct() {
                continue;
            }

            let name = room.display_name().await.unwrap_or(DisplayName::Empty);
            let tags = room.tags().await.unwrap_or_default();

            rooms.push((room.into(), name, tags));
        }

        for room in self.client.joined_rooms().into_iter() {
            if !room.is_direct() {
                continue;
            }

            let name = room.display_name().await.unwrap_or(DisplayName::Empty);
            let tags = room.tags().await.unwrap_or_default();

            rooms.push((room.into(), name, tags));
        }

        return rooms;
    }

    async fn active_rooms(&self) -> Vec<FetchedRoom> {
        let mut rooms = vec![];

        for room in self.client.invited_rooms().into_iter() {
            if room.is_space() || room.is_direct() {
                continue;
            }

            let name = room.display_name().await.unwrap_or(DisplayName::Empty);
            let tags = room.tags().await.unwrap_or_default();

            rooms.push((room.into(), name, tags));
        }

        for room in self.client.joined_rooms().into_iter() {
            if room.is_space() || room.is_direct() {
                continue;
            }

            let name = room.display_name().await.unwrap_or(DisplayName::Empty);
            let tags = room.tags().await.unwrap_or_default();

            rooms.push((room.into(), name, tags));
        }

        return rooms;
    }

    async fn members(&mut self, room_id: OwnedRoomId) -> IambResult<Vec<RoomMember>> {
        if let Some(room) = self.client.get_room(room_id.as_ref()) {
            Ok(room.active_members().await.map_err(IambError::from)?)
        } else {
            Err(IambError::UnknownRoom(room_id).into())
        }
    }

    async fn space_members(&mut self, space: OwnedRoomId) -> IambResult<Vec<OwnedRoomId>> {
        let mut req = SpaceHierarchyRequest::new(&space);
        req.limit = Some(1000u32.into());
        req.max_depth = Some(1u32.into());

        let resp = self.client.send(req, None).await.map_err(IambError::from)?;

        let rooms = resp.rooms.into_iter().map(|chunk| chunk.room_id).collect();

        Ok(rooms)
    }

    async fn spaces(&self) -> Vec<(MatrixRoom, DisplayName)> {
        let mut spaces = vec![];

        for room in self.client.invited_rooms().into_iter() {
            if !room.is_space() {
                continue;
            }

            let name = room.display_name().await.unwrap_or(DisplayName::Empty);

            spaces.push((room.into(), name));
        }

        for room in self.client.joined_rooms().into_iter() {
            if !room.is_space() {
                continue;
            }

            let name = room.display_name().await.unwrap_or(DisplayName::Empty);

            spaces.push((room.into(), name));
        }

        return spaces;
    }

    async fn typing_notice(&mut self, room_id: OwnedRoomId) {
        if let Some(room) = self.client.get_joined_room(room_id.as_ref()) {
            let _ = room.typing_notice(true).await;
        }
    }

    async fn verify(&self, action: VerifyAction, sas: SasVerification) -> IambResult<EditInfo> {
        match action {
            VerifyAction::Accept => {
                sas.accept().await.map_err(IambError::from)?;

                Ok(Some(InfoMessage::from("Accepted verification request")))
            },
            VerifyAction::Confirm => {
                if sas.is_done() || sas.is_cancelled() {
                    let msg = "Can only confirm in-progress verifications!";
                    let err = UIError::Failure(msg.into());

                    return Err(err);
                }

                sas.confirm().await.map_err(IambError::from)?;

                Ok(Some(InfoMessage::from("Confirmed verification")))
            },
            VerifyAction::Cancel => {
                if sas.is_done() || sas.is_cancelled() {
                    let msg = "Can only cancel in-progress verifications!";
                    let err = UIError::Failure(msg.into());

                    return Err(err);
                }

                sas.cancel().await.map_err(IambError::from)?;

                Ok(Some(InfoMessage::from("Cancelled verification")))
            },
            VerifyAction::Mismatch => {
                if sas.is_done() || sas.is_cancelled() {
                    let msg = "Can only cancel in-progress verifications!";
                    let err = UIError::Failure(msg.into());

                    return Err(err);
                }

                sas.mismatch().await.map_err(IambError::from)?;

                Ok(Some(InfoMessage::from("Cancelled verification")))
            },
        }
    }

    async fn verify_request(&self, user_id: OwnedUserId) -> IambResult<EditInfo> {
        let enc = self.client.encryption();

        match enc.get_user_identity(user_id.as_ref()).await.map_err(IambError::from)? {
            Some(identity) => {
                let methods = vec![VerificationMethod::SasV1];
                let request = identity.request_verification_with_methods(methods);
                let _req = request.await.map_err(IambError::from)?;
                let info = format!("Sent verification request to {user_id}");

                Ok(InfoMessage::from(info).into())
            },
            None => {
                let msg = format!("Could not find identity information for {user_id}");
                let err = UIError::Failure(msg);

                Err(err)
            },
        }
    }
}
