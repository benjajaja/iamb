use std::{
    convert::TryFrom,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
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
use ratatu_image::Resize;

use crate::{
    base::{AsyncProgramStore, ChatStore, IambError},
    config::ImagePreviewSize,
    message::ImageBackend,
};

pub struct PreviewSource {
    pub source: MediaSource,
    pub event_id: OwnedEventId,
}

impl TryFrom<&MessageLikeEvent<RoomMessageEventContent>> for PreviewSource {
    type Error = &'static str;
    fn try_from(ev: &MessageLikeEvent<RoomMessageEventContent>) -> Result<Self, Self::Error> {
        if let MessageLikeEvent::Original(ev) = &ev {
            if let MessageType::Image(c) = &ev.content.msgtype {
                Ok(PreviewSource {
                    source: c.source.clone(),
                    event_id: ev.event_id.clone(),
                })
            } else {
                Err("content message type is not image")
            }
        } else {
            Err("event is not original event")
        }
    }
}

impl Into<Rect> for &ImagePreviewSize {
    fn into(self) -> Rect {
        Rect::new(0, 0, self.width as _, self.height as _)
    }
}
impl From<Rect> for ImagePreviewSize {
    fn from(rect: Rect) -> Self {
        ImagePreviewSize { width: rect.width as _, height: rect.height as _ }
    }
}

pub fn spawn_insert_preview(
    store: AsyncProgramStore,
    room_id: OwnedRoomId,
    source: PreviewSource,
    media: Media,
    cache_dir: PathBuf,
) {
    tokio::spawn(async move {
        let event_id = source.event_id.clone();
        let img = download_or_cache(source, media, cache_dir)
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
                let ChatStore { rooms, picker, .. } = &mut locked.application;

                match picker
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
                        ))
                    })
                    .and_then(|(picker, msg)| {
                        let size = picker.private();
                        msg.image_backend = ImageBackend::Preparing(picker.private().clone());
                        picker
                            .new_static_fit(
                                img,
                                event_id.to_string().into(),
                                Resize::Fit,
                                size.into(),
                            )
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
    });
}

fn try_set_msg_preview_error(
    application: &mut ChatStore,
    room_id: OwnedRoomId,
    event_id: OwnedEventId,
    err: IambError,
) {
    let rooms = &mut application.rooms;

    match rooms
        .get_or_default(room_id.clone())
        .get_event_mut(&event_id)
        .ok_or_else(|| IambError::Preview("Message not found".to_string()))
    {
        Ok(msg) => msg.image_backend = ImageBackend::Error(format!("{err:?}")),
        Err(err) => eprintln!("{err:?}"),
    }
}

async fn download_or_cache(
    source: PreviewSource,
    media: Media,
    mut cache_path: PathBuf,
) -> Result<Vec<u8>, matrix_sdk::Error> {
    cache_path.push(Path::new("image_preview_downloads"));
    cache_path.push(Path::new(source.event_id.localpart()));

    match File::open(&cache_path) {
        Ok(mut f) => {
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer)?;
            Ok(buffer)
        },
        Err(_) => {
            match media
                .get_media_content(
                    &MediaRequest { source: source.source, format: MediaFormat::File },
                    true,
                )
                .await
            {
                Ok(buffer) => {
                    if let Err(err) =
                        File::create(&cache_path).and_then(|mut f| f.write_all(&buffer))
                    {
                        eprintln!("cache file write error ({:?}): {}", cache_path, err);
                    }
                    Ok(buffer)
                },
                Err(err) => Err(err),
            }
        },
    }
}
