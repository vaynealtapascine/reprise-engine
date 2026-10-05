//! Full decoding belongs exclusively to rendering backends.
use crate::assets::{ImageFormat, MAX_DECODE_PIXELS, image_header};
use std::io::Cursor;

pub(crate) fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let header = image_header(bytes).ok()?;
    let pixels = u64::from(header.width).checked_mul(u64::from(header.height))?;
    if pixels > MAX_DECODE_PIXELS {
        return None;
    }
    let size = usize::try_from(pixels.checked_mul(4)?).ok()?;
    let rgba = match header.format {
        ImageFormat::Png => {
            let mut decoder =
                png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: 128 << 20 });
            decoder
                .set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
            decoder.set_ignore_text_chunk(true);
            decoder.set_ignore_iccp_chunk(true);
            let mut reader = decoder.read_info().ok()?;
            if reader.info().width != header.width || reader.info().height != header.height {
                return None;
            }
            let output_size = reader.output_buffer_size()?;
            if output_size > size {
                return None;
            }
            let mut buffer = vec![0; output_size];
            let info = reader.next_frame(&mut buffer).ok()?;
            let input = buffer.get(..info.buffer_size())?;
            let channels = match info.color_type {
                png::ColorType::Grayscale => 1,
                png::ColorType::GrayscaleAlpha => 2,
                png::ColorType::Rgb => 3,
                png::ColorType::Rgba => 4,
                _ => return None,
            };
            if input.len() != usize::try_from(pixels).ok()?.checked_mul(channels)? {
                return None;
            }
            let mut out = Vec::with_capacity(size);
            for pixel in input.chunks_exact(channels) {
                match pixel {
                    [g] => out.extend_from_slice(&[*g, *g, *g, 255]),
                    [g, a] => out.extend_from_slice(&[*g, *g, *g, *a]),
                    [r, g, b] => out.extend_from_slice(&[*r, *g, *b, 255]),
                    [r, g, b, a] => out.extend_from_slice(&[*r, *g, *b, *a]),
                    _ => return None,
                }
            }
            out
        }
        ImageFormat::Jpeg => {
            use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
            let options = DecoderOptions::new_safe()
                .jpeg_set_out_colorspace(ColorSpace::RGBA)
                .jpeg_set_max_scans(256)
                .set_max_width(header.width as usize)
                .set_max_height(header.height as usize);
            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), options);
            decoder.decode_headers().ok()?;
            if decoder.output_buffer_size()? != size {
                return None;
            }
            let mut out = vec![0; size];
            decoder.decode_into(&mut out).ok()?;
            out
        }
    };
    (rgba.len() == size).then_some((header.width, header.height, rgba))
}
