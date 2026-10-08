//! Versioned authored image records (05, 33, 34). Pixels belong to the host.
use crate::{BlockKind, DocError, Document, LengthExpr, NodeId, get_str};
use serde::{Deserialize, Serialize};

/// Content-addressed image and optional authored physical size. The block's
/// collaborative text container is its alt text, including an empty string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub asset: String,
    pub width: Option<LengthExpr>,
    pub height: Option<LengthExpr>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    image: ImageData,
}

impl ImageData {
    pub fn new(asset: impl Into<String>) -> Self {
        Self {
            asset: asset.into(),
            width: None,
            height: None,
        }
    }
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.len() > 65536 {
            return Err(raw.into());
        }
        let record: Record = serde_json::from_str(raw).map_err(|_| raw.to_owned())?;
        if record.version != 1
            || record.image.asset.len() != 64
            || !record
                .image
                .asset
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(raw.into());
        }
        Ok(record.image)
    }
}

impl Document {
    pub fn set_image(&self, node: NodeId, image: &ImageData) -> Result<(), DocError> {
        let raw = serde_json::to_string(&Record {
            version: 1,
            image: image.clone(),
        })
        .map_err(|e| DocError::Store(e.to_string()))?;
        ImageData::parse(&raw).map_err(|_| DocError::Store("invalid image record".into()))?;
        self.set_image_record(node, &raw)
    }
    pub fn append_image(
        &self,
        style: &str,
        image: &ImageData,
        alt: &str,
    ) -> Result<NodeId, DocError> {
        let raw = serde_json::to_string(&Record {
            version: 1,
            image: image.clone(),
        })
        .map_err(|e| DocError::Store(e.to_string()))?;
        ImageData::parse(&raw).map_err(|_| DocError::Store("invalid image record".into()))?;
        let node = self.append_block(BlockKind::Image, style, alt)?;
        self.set_image_record(node, &raw)?;
        Ok(node)
    }

    /// Raw records remain available even when a newer version cannot be read.
    pub fn image_record(&self, node: NodeId) -> Result<Option<String>, DocError> {
        self.block(node)?;
        Ok(get_str(&self.meta_of(node)?, "image1"))
    }
    pub fn image(&self, node: NodeId) -> Result<ImageData, DocError> {
        let raw = self
            .image_record(node)?
            .ok_or(DocError::Malformed(node, "image record"))?;
        ImageData::parse(&raw).map_err(|_| DocError::Malformed(node, "image record"))
    }
    /// Store an opaque record without normalizing unknown fields. Does not commit.
    pub fn set_image_record(&self, node: NodeId, raw: &str) -> Result<(), DocError> {
        if self.block(node)?.kind != BlockKind::Image {
            return Err(DocError::Malformed(node, "not an image"));
        }
        self.meta_of(node)?.insert("image1", raw)?;
        Ok(())
    }
    /// Paste metadata on an invisible staged block, outside the undo history.
    pub fn stage_fragment_image(&self, node: NodeId, raw: &str) -> Result<(), DocError> {
        if !self.is_soft_deleted(node) {
            return Err(DocError::NoNode(node));
        }
        self.doc
            .set_next_commit_origin(crate::lifecycle::STAGE_ORIGIN);
        self.tree("content")
            .get_meta(node.node)?
            .insert("image1", raw)?;
        self.doc.commit();
        Ok(())
    }
}
