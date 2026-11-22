//! The protocol between the broker and its GPU process (UI-05, ADR 0068).
//!
//! The broker validates a renderer's surface, resolves its frame handles
//! from its own table and checks every layer's geometry; only then does it
//! hand the work to the GPU process, which composites and returns pixels.
//! The protocol is stateless: each [`GpuCommand::Composite`] carries
//! everything it needs, so a restarted GPU process has nothing stale to
//! reuse. The broker treats every reply as hostile: it must name the
//! request, keep the page's size and hold premultiplied pixels.

use crate::Message;
use crate::protocol::{FrameLayer, MAX_FRAMES, Surface};
use crate::wire::{DecodeError, Reader, Writer};

/// One frame's pixels.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Premultiplied RGBA, four bytes per pixel, row by row.
    pub pixels: Vec<u8>,
}

/// One composition: a page and the frames its layers name. Layer handle
/// `n` is `frames[n]` (the broker renumbers its own handles).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Composite {
    /// The request number, unique for the life of the process.
    pub id: u64,
    /// The page surface and its frame layers in paint order.
    pub page: Surface,
    /// The frames' pixels.
    pub frames: Vec<Image>,
}

/// Broker to GPU process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GpuCommand {
    /// Composite the frames into the page.
    Composite(Box<Composite>),
}

/// GPU process to broker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GpuEvent {
    /// The composited page.
    Composited {
        /// The request.
        id: u64,
        /// Width in pixels (the page's).
        width: u32,
        /// Height in pixels (the page's).
        height: u32,
        /// Premultiplied RGBA.
        pixels: Vec<u8>,
    },
    /// The request could not be composited (its input was invalid).
    Failed {
        /// The request.
        id: u64,
    },
}

fn put_layers(writer: &mut Writer, layers: &[FrameLayer]) {
    writer.u32(u32::try_from(layers.len()).unwrap_or(u32::MAX));
    for layer in layers {
        writer.u32(layer.handle);
        writer.u32(layer.x);
        writer.u32(layer.y);
        writer.u32(layer.width);
        writer.u32(layer.height);
        writer.bytes(&layer.coverage);
    }
}

fn get_layers(reader: &mut Reader<'_>) -> Result<Vec<FrameLayer>, DecodeError> {
    let count = reader.u32()? as usize;
    if count > MAX_FRAMES {
        return Err(DecodeError::OutOfRange);
    }
    (0..count)
        .map(|_| {
            Ok(FrameLayer {
                handle: reader.u32()?,
                x: reader.u32()?,
                y: reader.u32()?,
                width: reader.u32()?,
                height: reader.u32()?,
                coverage: reader.bytes()?,
            })
        })
        .collect()
}

impl Message for GpuCommand {
    fn encode(&self, writer: &mut Writer) {
        match self {
            Self::Composite(composite) => {
                writer.u16(1);
                writer.u64(composite.id);
                writer.u32(composite.page.width);
                writer.u32(composite.page.height);
                writer.bytes(&composite.page.pixels);
                put_layers(writer, &composite.page.layers);
                writer.u32(u32::try_from(composite.frames.len()).unwrap_or(u32::MAX));
                for frame in &composite.frames {
                    writer.u32(frame.width);
                    writer.u32(frame.height);
                    writer.bytes(&frame.pixels);
                }
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match reader.u16()? {
            1 => {
                let id = reader.u64()?;
                let width = reader.u32()?;
                let height = reader.u32()?;
                let pixels = reader.bytes()?;
                let layers = get_layers(reader)?;
                let count = reader.u32()? as usize;
                if count > MAX_FRAMES {
                    return Err(DecodeError::OutOfRange);
                }
                let frames = (0..count)
                    .map(|_| {
                        Ok(Image {
                            width: reader.u32()?,
                            height: reader.u32()?,
                            pixels: reader.bytes()?,
                        })
                    })
                    .collect::<Result<_, DecodeError>>()?;
                Ok(Self::Composite(Box::new(Composite {
                    id,
                    page: Surface {
                        width,
                        height,
                        pixels,
                        layers,
                    },
                    frames,
                })))
            }
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}

impl Message for GpuEvent {
    fn encode(&self, writer: &mut Writer) {
        match self {
            Self::Composited {
                id,
                width,
                height,
                pixels,
            } => {
                writer.u16(1);
                writer.u64(*id);
                writer.u32(*width);
                writer.u32(*height);
                writer.bytes(pixels);
            }
            Self::Failed { id } => {
                writer.u16(2);
                writer.u64(*id);
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match reader.u16()? {
            1 => Ok(Self::Composited {
                id: reader.u64()?,
                width: reader.u32()?,
                height: reader.u32()?,
                pixels: reader.bytes()?,
            }),
            2 => Ok(Self::Failed { id: reader.u64()? }),
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame};

    fn composite() -> GpuCommand {
        GpuCommand::Composite(Box::new(Composite {
            id: 7,
            page: Surface {
                width: 2,
                height: 1,
                pixels: vec![255; 8],
                layers: vec![FrameLayer {
                    handle: 0,
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 1,
                    coverage: vec![255],
                }],
            },
            frames: vec![Image {
                width: 1,
                height: 1,
                pixels: vec![0, 0, 255, 255],
            }],
        }))
    }

    #[test]
    fn messages_round_trip_and_reject_trailing_bytes() {
        let command = composite();
        let bytes = encode_frame(&command);
        assert_eq!(decode_frame::<GpuCommand>(&bytes).unwrap(), command);
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode_frame::<GpuCommand>(&longer).is_err());
        for event in [
            GpuEvent::Composited {
                id: 7,
                width: 2,
                height: 1,
                pixels: vec![1; 8],
            },
            GpuEvent::Failed { id: 9 },
        ] {
            assert_eq!(
                decode_frame::<GpuEvent>(&encode_frame(&event)).unwrap(),
                event
            );
        }
        assert!(decode_frame::<GpuEvent>(&[3, 0]).is_err());
    }

    #[test]
    fn layer_and_frame_counts_are_bounded_before_allocation() {
        let mut writer = Writer::new();
        writer.u16(1);
        writer.u64(1);
        writer.u32(1);
        writer.u32(1);
        writer.bytes(&[0; 4]);
        writer.u32(u32::MAX);
        let bytes = writer.into_bytes();
        assert!(matches!(
            decode_frame::<GpuCommand>(&bytes),
            Err(crate::IpcError::Malformed(DecodeError::OutOfRange))
        ));
    }
}
