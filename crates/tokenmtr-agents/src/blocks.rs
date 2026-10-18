//! `message.content` as agents write it: an array of content blocks, or a plain string (user
//! prompts) that is skipped.

use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{self, Deserializer, SeqAccess, Visitor};

/// The blocks of one message; empty when `content` is a string or `null`. `B` is the agent's
/// own partial block struct, so large block payloads are skipped without allocating.
#[derive(Debug)]
pub(crate) struct Blocks<B>(pub(crate) Vec<B>);

impl<B> Default for Blocks<B> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, B: Deserialize<'de>> Deserialize<'de> for Blocks<B> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BlocksVisitor<B>(PhantomData<B>);

        impl<'de, B: Deserialize<'de>> Visitor<'de> for BlocksVisitor<B> {
            type Value = Blocks<B>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string or an array of content blocks")
            }

            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(Blocks::default())
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Blocks::default())
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut blocks = Vec::new();
                while let Some(block) = seq.next_element::<B>()? {
                    blocks.push(block);
                }
                Ok(Blocks(blocks))
            }
        }

        deserializer.deserialize_any(BlocksVisitor(PhantomData))
    }
}
