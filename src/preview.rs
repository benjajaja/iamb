use std::{
    fs::File,
    io::{Read, Write},
    path::{PathBuf, Path},
};

use matrix_sdk::{
    media::{MediaFormat, MediaRequest},
    ruma::{
        events::{
            room::{
                message::{MessageType, RoomMessageEventContent},
                MediaSource,
            },
            MessageLikeEvent,
        },
        OwnedEventId,
        OwnedRoomId,
    },
    Media,
};
use modalkit::tui::layout::Rect;
use ratatui_image::{Resize, picker::Picker};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender, error::SendError};

use crate::{
    base::{AsyncProgramStore, ChatStore, IambError},
    config::{ImagePreviewSize, ImagePreviewProtocolValues, ImagePreviewValues},
    message::ImageBackend,
};

pub struct Previewer {
    pub picker: Picker,
    tx: UnboundedSender<PreviewMessage>,
}

impl Previewer {
    pub(crate) fn spawn(
        image_preview: Option<ImagePreviewValues>,
        cache_dir: PathBuf,
        store: AsyncProgramStore,
        media: Media,
    ) -> Option<Previewer> {
        if let Some(image_preview) = image_preview {
            let picker = match image_preview.protocol.as_ref() {
                Some(&ImagePreviewProtocolValues {
                    r#type: Some(backend),
                    font_size: Some(font_size),
                }) => Some(Picker::new(font_size, backend, None).unwrap()),
                #[cfg(not(target_os = "windows"))]
                Some(&ImagePreviewProtocolValues {
                    r#type: Some(backend),
                    font_size: None,
                }) => {
                    let mut picker = Picker::from_termios(None).unwrap();
                    picker.set(backend);
                    Some(picker)
                },
                #[cfg(not(target_os = "windows"))]
                _ => Some(Picker::from_termios(None).unwrap()),
                #[cfg(target_os = "windows")]
                _ => None,
            };
            let picker = match picker {
                Some(picker) => picker,
                None => {
                    return None;
                },
            };

            let (tx, mut rx) = unbounded_channel::<PreviewMessage>();

            tokio::spawn(async move {
                loop {
                    if let Some(PreviewMessage(room_id, event_id, source)) = rx.recv().await {
                        spawn_insert_preview(store.clone(), room_id, event_id, source, media.clone(), cache_dir.clone()).await;
                    }
                }
            });

            return Some(Previewer {
                picker,tx
            });
        }
        None
    }

    pub(crate) fn send(&self, room_id: OwnedRoomId, event_id: OwnedEventId, source: MediaSource) -> Result<(), SendError<PreviewMessage>>  {
        self.tx.send(PreviewMessage(room_id, event_id, source))
    }


}

pub struct PreviewMessage (
    OwnedRoomId,
    OwnedEventId,
    MediaSource,
);

pub fn source_from_event(ev: &MessageLikeEvent<RoomMessageEventContent>) -> Option<(OwnedEventId, MediaSource)> {
    if let MessageLikeEvent::Original(ev) = &ev {
        if let MessageType::Image(c) = &ev.content.msgtype {
            return Some((ev.event_id.clone(), c.source.clone()))
        }
    }
    None
}

impl From<ImagePreviewSize> for Rect {
    fn from(value: ImagePreviewSize) -> Self {
        Rect::new(0, 0, value.width as _, value.height as _)
    }
}
impl From<Rect> for ImagePreviewSize {
    fn from(rect: Rect) -> Self {
        ImagePreviewSize { width: rect.width as _, height: rect.height as _ }
    }
}

// Download and prepare the preview, and then lock the store to insert it.
pub async fn spawn_insert_preview(
    store: AsyncProgramStore,
    room_id: OwnedRoomId,
    event_id: OwnedEventId,
    source: MediaSource,
    media: Media,
    cache_dir: PathBuf,
) {
        let img = download_or_load(event_id.to_owned(), source, media, cache_dir)
            .await
            .map(std::io::Cursor::new)
            .map(image::io::Reader::new)
            .map_err(IambError::Matrix)
            .and_then(|reader| reader.with_guessed_format().map_err(IambError::IOError))
            .and_then(|reader| reader.decode().map_err(IambError::Image));

        match img {
            Err(err) => {
                try_set_msg_preview_error(
                    &mut store.lock().await.application,
                    room_id,
                    event_id,
                    err,
                );
            },
            Ok(img) => {
                let mut locked = store.lock().await;
                let ChatStore { rooms, previewer, settings, .. } = &mut locked.application;

                match previewer
                    .as_mut()
                    .ok_or_else(|| IambError::Preview("Picker is empty".to_string()))
                    .and_then(|picker| {
                        Ok((
                            picker,
                            rooms
                                .get_or_default(room_id.clone())
                                .get_event_mut(&event_id)
                                .ok_or_else(|| {
                                    IambError::Preview("Message not found".to_string())
                                })?,
                            settings.tunables.image_preview.clone().ok_or_else(|| {
                                IambError::Preview("image_preview settings not found".to_string())
                            })?,
                        ))
                    })
                    .and_then(|(previewer, msg, image_preview)| {
                        msg.image_backend = ImageBackend::Preparing(image_preview.size.clone());
                        previewer
                            .picker
                            .new_static_fit(img, image_preview.size.into(), Resize::Fit)
                            .map_err(|err| IambError::Preview(format!("{err:?}")))
                            .map(|backend| (backend, msg))
                    }) {
                    Err(err) => {
                        try_set_msg_preview_error(&mut locked.application, room_id, event_id, err);
                    },
                    Ok((backend, msg)) => {
                        msg.image_backend = ImageBackend::Loaded(backend);
                    },
                }
            },
        }
}

fn try_set_msg_preview_error(
    application: &mut ChatStore,
    room_id: OwnedRoomId,
    event_id: OwnedEventId,
    err: IambError,
) {
    let rooms = &mut application.rooms;

    match rooms
        .get_or_default(room_id)
        .get_event_mut(&event_id)
        .ok_or_else(|| IambError::Preview("Message not found".to_string()))
    {
        Ok(msg) => msg.image_backend = ImageBackend::Error(format!("{err:?}")),
        Err(_) => {
            // What can we do?
        },
    }
}

async fn download_or_load(
    event_id: OwnedEventId,
    source: MediaSource,
    media: Media,
    mut cache_path: PathBuf,
) -> Result<Vec<u8>, matrix_sdk::Error> {
    cache_path.push(Path::new(event_id.localpart()));

    match File::open(&cache_path) {
        Ok(mut f) => {
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer)?;
            Ok(buffer)
        },
        Err(_) => {
            media
                .get_media_content(
                    &MediaRequest { source, format: MediaFormat::File },
                    true,
                )
                .await
                .and_then(|buffer| {
                    if let Err(err) =
                        File::create(&cache_path).and_then(|mut f| f.write_all(&buffer))
                    {
                        return Err(err.into());
                    }
                    Ok(buffer)
            })
        },
    }
}
