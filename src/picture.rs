//! An image file, decoded and fitted to the pane on a thread of its own.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use image::ImageFormat;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};

/// The formats shown as pictures.
const SHOWN: [ImageFormat; 7] = [
    ImageFormat::Png,
    ImageFormat::Jpeg,
    ImageFormat::Gif,
    ImageFormat::WebP,
    ImageFormat::Bmp,
    ImageFormat::Ico,
    ImageFormat::Tiff,
];

/// The format of the image `bytes` hold, when it is one shown as a picture.
pub fn format(bytes: &[u8]) -> Option<ImageFormat> {
    image::guess_format(bytes).ok().filter(|format| SHOWN.contains(format))
}

pub fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Tiff => "TIFF",
        _ => "image",
    }
}

pub enum Stage {
    /// Being decoded, holding the channel the decoded picture will send its resizes down.
    Loading(Sender<ResizeRequest>),
    Ready {
        width: u32,
        height: u32,
        protocol: Box<ThreadProtocol>,
    },
    Failed(String),
}

pub struct Picture {
    pub format: ImageFormat,
    pub stage: Stage,
    reports: Receiver<Report>,
}

/// What the worker finished.
enum Report {
    Decoded(Result<Decoded, String>),
    Resized(Result<ResizeResponse, String>),
}

struct Decoded {
    width: u32,
    height: u32,
    protocol: StatefulProtocol,
}

impl Picture {
    /// Start decoding `bytes`, an image in `format`, for drawing with `picker`.
    pub fn load(bytes: Vec<u8>, format: ImageFormat, picker: Picker) -> Self {
        let (requests, jobs) = mpsc::channel();
        let (reporter, reports) = mpsc::channel();
        thread::spawn(move || work(bytes, format, &picker, &jobs, &reporter));
        Self { format, stage: Stage::Loading(requests), reports }
    }

    /// Take in what the worker finished since the last call.
    pub fn poll(&mut self) {
        let reports: Vec<Report> = self.reports.try_iter().collect();
        for report in reports {
            match (report, &mut self.stage) {
                (Report::Decoded(Ok(decoded)), Stage::Loading(requests)) => {
                    let protocol =
                        Box::new(ThreadProtocol::new(requests.clone(), Some(decoded.protocol)));
                    self.stage =
                        Stage::Ready { width: decoded.width, height: decoded.height, protocol };
                }
                (Report::Resized(Ok(response)), Stage::Ready { protocol, .. }) => {
                    protocol.update_resized_protocol(response);
                }
                (Report::Decoded(Err(error)) | Report::Resized(Err(error)), _) => {
                    self.stage = Stage::Failed(error);
                }
                _ => {}
            }
        }
    }

    /// Its width and height in pixels, once decoded.
    pub fn size(&self) -> Option<(u32, u32)> {
        match self.stage {
            Stage::Ready { width, height, .. } => Some((width, height)),
            Stage::Loading(_) | Stage::Failed(_) => None,
        }
    }
}

/// Decode `bytes` and report it, then resize and encode it for each request until the picture
/// stops asking or stops listening.
fn work(
    bytes: Vec<u8>,
    format: ImageFormat,
    picker: &Picker,
    jobs: &Receiver<ResizeRequest>,
    reporter: &Sender<Report>,
) {
    let decoded = image::load_from_memory_with_format(&bytes, format);
    drop(bytes);
    let decoded = decoded.map_err(|error| error.to_string()).map(|image| Decoded {
        width: image.width(),
        height: image.height(),
        protocol: picker.new_resize_protocol(image),
    });
    if reporter.send(Report::Decoded(decoded)).is_err() {
        return;
    }
    for job in jobs {
        let resized = job.resize_encode().map_err(|error| error.to_string());
        if reporter.send(Report::Resized(resized)).is_err() {
            return;
        }
    }
}

/// A black PNG `width` by `height` pixels.
#[cfg(test)]
pub fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn settle(picture: &mut Picture) {
        let start = Instant::now();
        while matches!(picture.stage, Stage::Loading(_)) && start.elapsed() < Duration::from_secs(5)
        {
            picture.poll();
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn an_image_is_known_by_its_content_and_text_is_not() {
        assert_eq!(format(&png(1, 1)), Some(ImageFormat::Png));
        assert_eq!(format(b"fn main() {}"), None);
    }

    #[test]
    fn a_picture_decodes_on_its_own_thread() {
        let mut picture = Picture::load(png(3, 2), ImageFormat::Png, Picker::halfblocks());
        settle(&mut picture);
        assert_eq!(picture.size(), Some((3, 2)));
    }

    #[test]
    fn a_picture_that_wont_decode_says_why() {
        let mut broken = png(3, 2);
        broken.truncate(20);
        let mut picture = Picture::load(broken, ImageFormat::Png, Picker::halfblocks());
        settle(&mut picture);
        assert!(matches!(&picture.stage, Stage::Failed(reason) if !reason.is_empty()));
    }
}
