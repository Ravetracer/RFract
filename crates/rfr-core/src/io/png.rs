// SPDX-License-Identifier: LGPL-3.0-or-later

//! PNG images with the document in an iTXt chunk (keyword [`KEYWORD`]): every rendered image
//! opens again as its parameters.

use std::io::BufWriter;
use std::path::Path;

use crate::doc::Document;

pub const KEYWORD: &str = "rfract-document";

/// Writes 8-bit RGBA pixels, with `doc` embedded when given.
pub fn write_rgba8(path: &Path, width: u32, height: u32, rgba: &[u8], doc: Option<&Document>) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = std::fs::File::create(path).map_err(|e| err(&e))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Some(d) = doc {
        enc.add_itxt_chunk(KEYWORD.into(), d.to_json()).map_err(|e| err(&e))?;
    }
    enc.add_text_chunk("Software".into(), crate::name_version()).map_err(|e| err(&e))?;
    let mut w = enc.write_header().map_err(|e| err(&e))?;
    w.write_image_data(rgba).map_err(|e| err(&e))?;
    w.finish().map_err(|e| err(&e))
}

/// A PNG written a band of rows at a time (posters larger than memory likes), with the
/// document inside like [`write_rgba8`].
pub struct RowWriter {
    stream: png::StreamWriter<'static, BufWriter<std::fs::File>>,
    path: std::path::PathBuf,
}

impl RowWriter {
    pub fn new(path: &Path, width: u32, height: u32, doc: Option<&Document>) -> Result<RowWriter, String> {
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let file = std::fs::File::create(path).map_err(|e| err(&e))?;
        let mut enc = png::Encoder::new(BufWriter::new(file), width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        if let Some(d) = doc {
            enc.add_itxt_chunk(KEYWORD.into(), d.to_json()).map_err(|e| err(&e))?;
        }
        enc.add_text_chunk("Software".into(), crate::name_version()).map_err(|e| err(&e))?;
        let w = enc.write_header().map_err(|e| err(&e))?;
        let stream = w.into_stream_writer().map_err(|e| err(&e))?;
        Ok(RowWriter { stream, path: path.to_path_buf() })
    }

    /// Writes whole rows (RGBA8), top to bottom.
    pub fn write_rows(&mut self, rgba: &[u8]) -> Result<(), String> {
        use std::io::Write;
        self.stream.write_all(rgba).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    pub fn finish(self) -> Result<(), String> {
        let p = self.path.clone();
        self.stream.finish().map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// The pixels of an 8-bit RGBA PNG (as RFract writes them): width, height, RGBA.
pub fn read_rgba8(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = std::fs::File::open(path).map_err(|e| err(&e))?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().map_err(|e| err(&e))?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or_else(|| err(&"image too large"))?];
    let info = reader.next_frame(&mut buf).map_err(|e| err(&e))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(err(&"not an 8-bit RGBA image"));
    }
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

/// The document embedded in a PNG written by RFract.
pub fn read_document(path: &Path) -> Result<Document, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dec = png::Decoder::new(std::io::BufReader::new(file));
    let reader = dec.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let info = reader.info();
    let text = info
        .utf8_text
        .iter()
        .find(|c| c.keyword == KEYWORD)
        .and_then(|c| c.get_text().ok())
        .or_else(|| info.uncompressed_latin1_text.iter().find(|c| c.keyword == KEYWORD).map(|c| c.text.clone()))
        .ok_or_else(|| format!("{}: the image holds no RFract document", path.display()))?;
    Document::from_json(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_survives_the_png() {
        let dir = std::env::temp_dir().join(format!("rfract-png-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let mut doc = Document::default();
        doc.view.radius = 0.0123.into();
        write_rgba8(&path, 2, 1, &[1, 2, 3, 255, 4, 5, 6, 255], Some(&doc)).unwrap();
        assert_eq!(read_document(&path).unwrap(), doc);
        std::fs::remove_dir_all(&dir).ok();
    }
}
