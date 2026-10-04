//! Host-owned, content-addressed resources and bounded integer header inspection.
use reprise_geom::{Length, div_round};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const MAX_ASSET_BYTES: usize = 64 << 20;
pub const MAX_HEADER_BYTES: usize = 1 << 20;
pub const MAX_HEADER_PARTS: usize = 512;
pub const MAX_DECODE_PIXELS: u64 = 16 << 20;

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;

#[derive(Clone, Debug, Default)]
pub struct AssetStore {
    bytes: BTreeMap<String, Arc<[u8]>>,
}

impl AssetStore {
    pub fn insert(&mut self, bytes: impl Into<Arc<[u8]>>) -> Result<String, &'static str> {
        let bytes = bytes.into();
        if bytes.len() > MAX_ASSET_BYTES {
            return Err("asset byte limit");
        }
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.bytes.insert(hash.clone(), bytes);
        Ok(hash)
    }
    pub fn get(&self, hash: &str) -> Option<&[u8]> {
        self.bytes.get(hash).map(AsRef::as_ref)
    }
    pub fn remove(&mut self, hash: &str) -> Option<Arc<[u8]>> {
        self.bytes.remove(hash)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    Invalid,
    Unsupported,
    Limit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageHeader {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// Physical size in points, respecting PNG pHYs or JPEG JFIF/EXIF density.
    /// Without absolute density, one pixel is 3/4 point (96 DPI).
    pub physical_width: Length,
    pub physical_height: Length,
    // Keep the density-adjusted ratio before Length rounding or saturation.
    aspect: (u128, u128),
}

impl ImageHeader {
    /// Preserve the physical aspect even when intrinsic Lengths round to zero
    /// or saturate. Header dimensions and densities keep this ratio bounded.
    pub fn height_for_width(self, width: Length) -> Length {
        proportion(width, self.aspect.1, self.aspect.0)
    }

    pub fn width_for_height(self, height: Length) -> Length {
        proportion(height, self.aspect.0, self.aspect.1)
    }
}

fn proportion(size: Length, numerator: u128, denominator: u128) -> Length {
    let n = (size.0.max(0) as u128).saturating_mul(numerator);
    Length((n.saturating_add(denominator / 2) / denominator.max(1)).min(i32::MAX as u128) as i32)
}

fn length(pixels: u32, numerator: u32, denominator: u64) -> Length {
    let n = i64::from(pixels)
        .saturating_mul(i64::from(numerator))
        .saturating_mul(1024);
    Length(
        div_round(n, denominator.min(i64::MAX as u64) as i64).clamp(0, i64::from(i32::MAX)) as i32,
    )
}
fn u32be(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?))
}
fn u16be(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(..2)?.try_into().ok()?))
}

/// Scans metadata only, stopping at IDAT/SOS. Limits bound both scan distance
/// and chunk/marker count; no allocation depends on pixel dimensions.
pub fn image_header(bytes: &[u8]) -> Result<ImageHeader, HeaderError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else if bytes.starts_with(b"\xff\xd8") {
        jpeg(bytes)
    } else {
        Err(HeaderError::Unsupported)
    }
}

fn png(bytes: &[u8]) -> Result<ImageHeader, HeaderError> {
    let invalid = HeaderError::Invalid;
    let mut pos = 8usize;
    let mut dimensions = None;
    let mut density = None;
    for part in 0..MAX_HEADER_PARTS {
        if pos > MAX_HEADER_BYTES.saturating_sub(12) {
            return Err(HeaderError::Limit);
        }
        let n = u32be(bytes.get(pos..).ok_or(invalid)?).ok_or(invalid)? as usize;
        let kind = bytes.get(pos + 4..pos + 8).ok_or(invalid)?;
        if kind == b"IDAT" {
            let (width, height) = dimensions.ok_or(invalid)?;
            let (physical_width, physical_height) = match density {
                Some((x, y)) => (
                    length(width, 720000, u64::from(x) * 254),
                    length(height, 720000, u64::from(y) * 254),
                ),
                None => (length(width, 3, 4), length(height, 3, 4)),
            };
            return Ok(ImageHeader {
                format: ImageFormat::Png,
                width,
                height,
                physical_width,
                physical_height,
                aspect: match density {
                    Some((x, y)) => (
                        u128::from(width).saturating_mul(u128::from(y)),
                        u128::from(height).saturating_mul(u128::from(x)),
                    ),
                    None => (u128::from(width), u128::from(height)),
                },
            });
        }
        let end = pos
            .checked_add(n)
            .and_then(|v| v.checked_add(12))
            .ok_or(HeaderError::Limit)?;
        if end > MAX_HEADER_BYTES {
            return Err(HeaderError::Limit);
        }
        let data = bytes.get(pos + 8..end - 4).ok_or(invalid)?;
        let expected = u32be(bytes.get(end - 4..end).ok_or(invalid)?).ok_or(invalid)?;
        // Metadata CRC validation is bounded by MAX_HEADER_BYTES.
        let mut crc = u32::MAX;
        for b in kind.iter().chain(data) {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (if crc & 1 == 1 { 0xedb88320 } else { 0 });
            }
        }
        if !crc != expected {
            return Err(invalid);
        }
        if part == 0 && (kind != b"IHDR" || n != 13) {
            return Err(invalid);
        }
        if kind == b"IHDR" {
            if dimensions.is_some() || n != 13 {
                return Err(invalid);
            }
            let width = u32be(data).ok_or(invalid)?;
            let height = u32be(data.get(4..).ok_or(invalid)?).ok_or(invalid)?;
            if width == 0
                || height == 0
                || width > i32::MAX as u32
                || height > i32::MAX as u32
                || data.get(10) != Some(&0)
                || data.get(11) != Some(&0)
                || !matches!(data.get(12), Some(0 | 1))
            {
                return Err(invalid);
            }
            let valid_depth = match data.get(9) {
                Some(0) => matches!(data.get(8), Some(1 | 2 | 4 | 8 | 16)),
                Some(2 | 4 | 6) => matches!(data.get(8), Some(8 | 16)),
                Some(3) => matches!(data.get(8), Some(1 | 2 | 4 | 8)),
                _ => false,
            };
            if !valid_depth {
                return Err(invalid);
            }
            dimensions = Some((width, height));
        } else if kind == b"pHYs" {
            if n != 9 {
                return Err(invalid);
            }
            let x = u32be(data).ok_or(invalid)?;
            let y = u32be(data.get(4..).ok_or(invalid)?).ok_or(invalid)?;
            if data.get(8) == Some(&1) && x != 0 && y != 0 {
                density = Some((x, y));
            }
        } else if kind == b"IEND" {
            return Err(invalid);
        }
        pos = end;
    }
    Err(HeaderError::Limit)
}

fn jpeg(bytes: &[u8]) -> Result<ImageHeader, HeaderError> {
    let invalid = HeaderError::Invalid;
    let mut pos = 2usize;
    let mut dimensions = None;
    let mut density = None;
    let mut exif = None;
    for _ in 0..MAX_HEADER_PARTS {
        if pos >= MAX_HEADER_BYTES {
            return Err(HeaderError::Limit);
        }
        if bytes.get(pos) != Some(&255) {
            return Err(invalid);
        }
        while bytes.get(pos) == Some(&255) {
            pos = pos.checked_add(1).ok_or(HeaderError::Limit)?;
            if pos >= MAX_HEADER_BYTES {
                return Err(HeaderError::Limit);
            }
        }
        let marker = *bytes.get(pos).ok_or(invalid)?;
        pos = pos.checked_add(1).ok_or(HeaderError::Limit)?;
        if matches!(marker, 0 | 0xd8 | 0xd9) {
            return Err(invalid);
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let n = usize::from(u16be(bytes.get(pos..).ok_or(invalid)?).ok_or(invalid)?);
        if n < 2 {
            return Err(invalid);
        }
        let end = pos.checked_add(n).ok_or(HeaderError::Limit)?;
        if end > MAX_HEADER_BYTES {
            return Err(HeaderError::Limit);
        }
        let data = bytes.get(pos + 2..end).ok_or(invalid)?;
        if marker == 0xe1 && data.starts_with(b"Exif\0\0") {
            exif = exif_density(data.get(6..).ok_or(invalid)?)?;
        }
        if marker == 0xe0 && data.starts_with(b"JFIF\0") {
            let unit = *data.get(7).ok_or(invalid)?;
            let x = u16be(data.get(8..).ok_or(invalid)?).ok_or(invalid)?;
            let y = u16be(data.get(10..).ok_or(invalid)?).ok_or(invalid)?;
            if matches!(unit, 1 | 2) && x != 0 && y != 0 {
                density = Some((unit, x, y));
            }
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let height = u32::from(u16be(data.get(1..).ok_or(invalid)?).ok_or(invalid)?);
            let width = u32::from(u16be(data.get(3..).ok_or(invalid)?).ok_or(invalid)?);
            let components = usize::from(*data.get(5).ok_or(invalid)?);
            if width == 0 || height == 0 || components == 0 || data.len() != 6 + 3 * components {
                return Err(invalid);
            }
            dimensions = Some((width, height));
        }
        if marker == 0xda {
            let (width, height) = dimensions.ok_or(invalid)?;
            let (physical_width, physical_height) = match density {
                Some((1, x, y)) => (
                    length(width, 72, u64::from(x)),
                    length(height, 72, u64::from(y)),
                ),
                Some((_, x, y)) => (
                    length(width, 7200, u64::from(x) * 254),
                    length(height, 7200, u64::from(y) * 254),
                ),
                None => match exif {
                    Some((unit, x, y)) => (
                        density_length(width, unit, x),
                        density_length(height, unit, y),
                    ),
                    None => (length(width, 3, 4), length(height, 3, 4)),
                },
            };
            return Ok(ImageHeader {
                format: ImageFormat::Jpeg,
                width,
                height,
                physical_width,
                physical_height,
                aspect: match density {
                    Some((_, x, y)) => (
                        u128::from(width).saturating_mul(u128::from(y)),
                        u128::from(height).saturating_mul(u128::from(x)),
                    ),
                    None => match exif {
                        Some((_, (xn, xd), (yn, yd))) => (
                            u128::from(width)
                                .saturating_mul(u128::from(xd))
                                .saturating_mul(u128::from(yn)),
                            u128::from(height)
                                .saturating_mul(u128::from(yd))
                                .saturating_mul(u128::from(xn)),
                        ),
                        None => (u128::from(width), u128::from(height)),
                    },
                },
            });
        }
        pos = end;
    }
    Err(HeaderError::Limit)
}

type Rational = (u32, u32);
type ExifDensity = (u16, Rational, Rational);

fn density_length(pixels: u32, unit: u16, (numerator, denominator): Rational) -> Length {
    let (n, d) = if unit == 2 {
        (
            u128::from(pixels) * 1024 * 72 * u128::from(denominator),
            u128::from(numerator),
        )
    } else {
        (
            u128::from(pixels) * 1024 * 7200 * u128::from(denominator),
            u128::from(numerator) * 254,
        )
    };
    Length(((n + d / 2) / d.max(1)).min(i32::MAX as u128) as i32)
}

/// Only the primary TIFF IFD's resolution fields are read. No recursive IFD
/// traversal, thumbnails, orientation changes, or pixel allocation occurs.
fn exif_density(bytes: &[u8]) -> Result<Option<ExifDensity>, HeaderError> {
    let invalid = HeaderError::Invalid;
    let little = match bytes.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err(invalid),
    };
    let short = |offset: usize| -> Option<u16> {
        let value = bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(value)
        } else {
            u16::from_be_bytes(value)
        })
    };
    let long = |offset: usize| -> Option<u32> {
        let value = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(value)
        } else {
            u32::from_be_bytes(value)
        })
    };
    if short(2) != Some(42) {
        return Err(invalid);
    }
    let offset = usize::try_from(long(4).ok_or(invalid)?).map_err(|_| invalid)?;
    let count = usize::from(short(offset).ok_or(invalid)?);
    if count > MAX_HEADER_PARTS {
        return Err(HeaderError::Limit);
    }
    let mut x = None;
    let mut y = None;
    let mut unit = 2; // TIFF default is inches.
    for entry in 0..count {
        let at = offset
            .checked_add(2)
            .and_then(|v| v.checked_add(entry.checked_mul(12)?))
            .ok_or(invalid)?;
        bytes
            .get(at..at.checked_add(12).ok_or(invalid)?)
            .ok_or(invalid)?;
        let tag = short(at).ok_or(invalid)?;
        if matches!(tag, 0x011a | 0x011b) {
            if short(at + 2) != Some(5) || long(at + 4) != Some(1) {
                return Err(invalid);
            }
            let pos = usize::try_from(long(at + 8).ok_or(invalid)?).map_err(|_| invalid)?;
            let n = long(pos).ok_or(invalid)?;
            let d = long(pos.checked_add(4).ok_or(invalid)?).ok_or(invalid)?;
            if n == 0 || d == 0 {
                return Err(invalid);
            }
            if tag == 0x011a {
                x = Some((n, d));
            } else {
                y = Some((n, d));
            }
        } else if tag == 0x0128 {
            if short(at + 2) != Some(3) || long(at + 4) != Some(1) {
                return Err(invalid);
            }
            unit = short(at + 8).ok_or(invalid)?;
        }
    }
    Ok(match (unit, x, y) {
        (2 | 3, Some(x), Some(y)) => Some((unit, x, y)),
        _ => None,
    })
}
