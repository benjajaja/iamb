use std::path::{Path, PathBuf};

use image::{imageops::FilterType, DynamicImage, GenericImageView};
use matrix_sdk::{
    media::{MediaFormat, MediaRequest},
    ruma::{
        events::{
            room::{
                message::{MessageType, RoomMessageEventContent},
                MediaSource,
            },
            SyncMessageLikeEvent,
        },
        EventId,
        OwnedEventId,
    },
    Media,
};
use modalkit::editing::action::UIError;
use sixel_rs::{
    encoder::{Encoder, QuickFrameBuilder},
    optflags::EncodePolicy,
};

use crate::{
    base::{IambError, IambResult},
    config::ApplicationSettings,
};

pub fn get_attachment_source(
    ev: &SyncMessageLikeEvent<RoomMessageEventContent>,
) -> Option<(MediaSource, OwnedEventId)> {
    if let SyncMessageLikeEvent::Original(ev) = &ev {
        if let MessageType::Image(c) = &ev.content.msgtype {
            Some((c.source.clone(), ev.event_id.clone()))
        } else {
            None
        }
    } else {
        None
    }
}

pub async fn download(
    media: &Media,
    source: MediaSource,
    event_id: OwnedEventId,
    settings: &ApplicationSettings,
) -> IambResult<PathBuf> {
    let filename = cache_path(&settings.dirs.image_preview_cache, &event_id);
    if filename.exists() {
        return Ok(filename);
    }

    let req = MediaRequest { source, format: MediaFormat::File };
    let bytes = media.get_media_content(&req, true).await.map_err(IambError::from)?;

    let buf = std::io::Cursor::new(bytes);
    let img = image::io::Reader::new(buf)
        .with_guessed_format()
        .map_err(IambError::from)?
        .decode()
        .map_err(IambError::from)?;

    let (w, h) = find_fit(&img, settings.tunables.image_preview.image_height);
    let resized_img = img.resize_exact(w, h, FilterType::Triangle);
    let rgba = resized_img.to_rgba8();
    let raw = rgba.as_raw();

    let encoder = Encoder::new().map_err(IambError::from)?;

    encoder.set_output(&filename).map_err(IambError::from)?;

    encoder.set_encode_policy(EncodePolicy::Fast).map_err(IambError::from)?;
    let frame = QuickFrameBuilder::new()
        .width(w as usize)
        .height(h as usize)
        .format(sixel_rs::sys::PixelFormat::RGBA8888)
        .pixels(raw.to_vec());

    encoder.encode_bytes(frame).map_err(IambError::from)?;
    Ok(filename)
}

fn find_fit(img: &DynamicImage, height: u16) -> (u32, u32) {
    let (img_width, img_height) = img.dimensions();
    let (w, h) = fit_dimensions(img_width, img_height, img_width, height as u32);
    (std::cmp::min(w, 1000), h)
}

fn fit_dimensions(width: u32, height: u32, bound_width: u32, bound_height: u32) -> (u32, u32) {
    if width <= bound_width && height <= bound_height {
        return (width, height);
    }

    let ratio = width * bound_height;
    let nratio = bound_width * height;

    let use_width = nratio <= ratio;
    let intermediate = if use_width {
        height * bound_width / width
    } else {
        width * bound_height / height
    };

    if use_width {
        (bound_width, std::cmp::max(1, intermediate))
    } else {
        (intermediate, std::cmp::max(1, bound_height))
    }
}

pub fn load_file_from_path(path: PathBuf) -> IambResult<String> {
    if !path.exists() {
        return Err(UIError::Failure(format!("Sixel does not exist: {path:?}")));
    }

    Ok(std::fs::read_to_string(path)?)
}

pub fn load_file_from_event_id(path: &Path, event_id: &EventId) -> Option<String> {
    let path = cache_path(path, event_id);
    load_file_from_path(path).ok()
}

fn cache_path(path: &Path, event_id: &EventId) -> PathBuf {
    path.join(PathBuf::from(format!("{event_id}.sixel")))
}

pub fn placeholder_text(_height: u16) -> String {
    return r#"









"#
    .to_owned();
}

#[derive(Clone)]
pub struct Sixel {
    pub data: String,
    pub height: u16,
}

impl Sixel {}
