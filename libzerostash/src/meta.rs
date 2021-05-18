use crate::compress;
use crate::object::{ObjectId, WriteObject};

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::Cursor;

type Encoder = compress::Encoder<WriteObject>;
type Decoder<'b> =
    serde_cbor::Deserializer<serde_cbor::de::IoRead<compress::Decoder<Cursor<&'b [u8]>>>>;
pub type ObjectIndex = HashMap<Field, HashSet<ObjectId>>;

// Header size max 512b
const HEADER_SIZE: usize = 512;

mod reader;
mod writer;

pub use reader::{ReadError, Reader};
pub use writer::{WriteError, Writer};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum MetaObjectHeader {
    V1 {
        next_object: Option<ObjectId>,
        offsets: Vec<FieldOffset>,
        end: usize,
    },
}

impl MetaObjectHeader {
    fn new(
        next_object: Option<ObjectId>,
        offsets: impl AsRef<[FieldOffset]>,
        end: usize,
    ) -> MetaObjectHeader {
        MetaObjectHeader::V1 {
            offsets: offsets.as_ref().to_vec(),
            next_object,
            end,
        }
    }

    pub fn next_object(&self) -> Option<ObjectId> {
        match self {
            MetaObjectHeader::V1 {
                ref next_object, ..
            } => *next_object,
        }
    }

    pub fn fields(&self) -> Vec<Field> {
        match self {
            MetaObjectHeader::V1 { ref offsets, .. } => {
                offsets.iter().map(FieldOffset::as_field).collect()
            }
        }
    }

    fn end(&self) -> usize {
        match self {
            MetaObjectHeader::V1 { ref end, .. } => *end,
        }
    }

    fn get_offset(&self, field: &str) -> Option<u32> {
        match self {
            MetaObjectHeader::V1 { ref offsets, .. } => {
                for fo in offsets.iter() {
                    if fo.as_field() == field {
                        return Some(fo.into());
                    }
                }
                None
            }
        }
    }
}

#[async_trait]
pub trait FieldWriter: Send {
    async fn write_next(&mut self, obj: impl Serialize + Send + 'async_trait);
}

#[async_trait]
pub trait FieldReader<T>: Send {
    async fn read_next(&mut self) -> Result<T, Box<dyn Error>>;
}

#[async_trait]
impl<'b, T> FieldReader<T> for Decoder<'b>
where
    T: DeserializeOwned,
{
    async fn read_next(&mut self) -> Result<T, Box<dyn Error>> {
        Ok(T::deserialize(self)?)
    }
}

pub type Field = String;
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct FieldOffset(u32, String);

impl From<&FieldOffset> for u32 {
    fn from(fo: &FieldOffset) -> u32 {
        fo.0
    }
}

impl FieldOffset {
    pub fn new(offs: u32, f: Field) -> Self {
        FieldOffset(offs, f)
    }

    fn as_field(&self) -> Field {
        self.1.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn can_deserialize_fields() {
        use crate::backends;
        use crate::chunks::{self, ChunkPointer};
        use crate::crypto::{self, CryptoDigest};
        use crate::meta;
        use crate::object::ObjectId;

        use secrecy::Secret;
        use std::sync::Arc;

        let key = Secret::new(*b"abcdef1234567890abcdef1234567890");

        let crypto = crypto::ObjectOperations::new(key);
        let storage = Arc::new(backends::test::InMemoryBackend::default());
        let oid = ObjectId::new(&crypto);
        let mut mw = meta::Writer::new(oid, storage.clone(), crypto.clone()).unwrap();

        let chunks = chunks::ChunkIndex::default();
        chunks
            .entry(CryptoDigest::default())
            .or_insert_with(|| ChunkPointer::default());

        mw.write_field("chunks", &chunks).await;
        mw.seal_and_store().await;

        let mut mr = meta::Reader::new(storage, crypto);
        let objects = mw.objects().get("chunks").unwrap();
        assert_eq!(objects.len(), 1);

        for id in objects.iter() {
            mr.open(&id).await.unwrap();
        }

        let mut chunks_restore = chunks::ChunkIndex::default();
        mr.read_into("chunks", &mut chunks_restore).await.unwrap();

        assert_eq!(chunks_restore.len(), 1);
    }
}
