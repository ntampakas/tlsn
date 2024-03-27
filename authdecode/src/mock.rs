use crate::{
    bitid::{Id, IdSet},
    encodings::{ActiveEncodings, Encoding, FullEncodings},
    prover::{EncodingVerifier, EncodingVerifierError},
    utils::{boolvec_to_u8vec, u8vec_to_boolvec, u8vec_to_boolvec_no_pad},
    verifier::{EncodingProvider, EncodingProviderError},
    InitData,
};

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, marker::PhantomData, ops::Range};

#[derive(Clone, PartialEq, Serialize, Deserialize, Default)]
/// The direction of the transcript.
pub enum Direction {
    #[default]
    Sent,
    Received,
}

/// Ids of transcript bits. Each bit is uniquely identified by the transcript's direction and
/// the bit's index in the transcript.
/// Ranges may overlap.
#[derive(Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MockBitIds {
    direction: Direction,
    /// Ranges of bits in the transcript. The ranges may overlap.
    ranges: Vec<Range<usize>>,
}

impl MockBitIds {
    pub fn new(direction: Direction, ranges: Vec<Range<usize>>) -> Self {
        Self { direction, ranges }
    }

    /// Returns the id of the bit at the given offset of the transcript.
    ///
    /// # Panics
    fn encode_bit_id(&self, offset: usize) -> Id {
        // All values are encoded in MSB-first order.
        // The first bit encodes the direction, the remaining 63 bits encode
        // the offset in the transcript.
        let mut id = vec![false; 64];
        let encoded_direction = if self.direction == Direction::Sent {
            [false]
        } else {
            [true]
        };

        assert!(offset < 1 << 63);

        let encoded_offset = u8vec_to_boolvec_no_pad(&offset.to_be_bytes());

        id[0..1].copy_from_slice(&encoded_direction);
        id[1 + (63 - encoded_offset.len())..].copy_from_slice(&encoded_offset);

        Id(u64::from_be_bytes(
            boolvec_to_u8vec(&id).try_into().unwrap(),
        ))
    }

    /// Decodes bit id into its direction and offset in the transcript.
    fn decode_bit_id(&self, id: Id) -> (Direction, usize) {
        let encoding = u8vec_to_boolvec(&id.0.to_be_bytes());
        let direction_encoding = &encoding[0..1];

        let direction = if direction_encoding == [false] {
            Direction::Sent
        } else {
            Direction::Received
        };

        let offset_encoding = &encoding[1..];
        let offset = usize::from_be_bytes(boolvec_to_u8vec(offset_encoding).try_into().unwrap());

        (direction, offset)
    }
}
impl IdSet for MockBitIds {
    fn drain_front(&mut self, mut count: usize) -> Self {
        let mut drained_ranges: Vec<Range<usize>> = Vec::new();

        while count > 0 {
            if self.ranges.is_empty() {
                // Nothing more to drain.
                break;
            }
            let mut range = self.ranges.remove(0);
            let range_len = range.len();

            let mut yielded = range.by_ref().take(count);
            // It is safe to `unwrap()` since the iterator will contain at least 1 element.
            let min = yielded.next().unwrap();
            let max = yielded.last().unwrap() + 1;
            let drained = Range {
                start: min,
                end: max,
            };
            drained_ranges.push(drained.clone());

            // If some elements are still present in the iter, then the range was partially
            // processed. Put back the unprocessed subrange.
            if range_len - drained.len() > 0 {
                let min2 = range.next().unwrap();
                let max2 = range.last().unwrap() + 1;
                self.ranges.push(Range {
                    start: min2,
                    end: max2,
                });
                break;
            }

            count -= drained.len();
        }

        // TODO combine adjacent ranges
        Self {
            direction: self.direction.clone(),
            ranges: drained_ranges,
        }
    }

    fn id(&self, index: usize) -> Id {
        let len = self.len();
        assert!(len > index);
        // How many indices already checked.
        let mut count_checked = 0;
        let mut value_at_index: Option<usize> = None;

        // Find which range the `index` is located in.
        for r in &self.ranges {
            if count_checked + r.len() > index {
                // Offset of the `index` from the start of this range.
                let offet = index - count_checked;
                value_at_index = Some(r.start + offet);
                break;
            }
            count_checked += r.len();
        }

        // It is safe to unwrap since the index is checked to be in the collection.
        self.encode_bit_id(value_at_index.unwrap())
    }

    fn ids(&self) -> Vec<Id> {
        (0..self.len()).map(|idx| self.id(idx)).collect::<Vec<_>>()
    }

    fn len(&self) -> usize {
        self.ranges.iter().map(|r| r.len()).sum()
    }

    fn new_from_iter<I: IntoIterator<Item = Self>>(iter: I) -> Self {
        let mut direction = None;
        let ranges = iter
            .into_iter()
            .flat_map(|i| {
                if let Some(dir) = &direction {
                    assert!(dir == &i.direction)
                } else {
                    // On first iteration, set the direction.
                    direction = Some(i.direction)
                }
                i.ranges
            })
            .collect::<Vec<_>>();

        Self {
            direction: direction.unwrap(),
            // TODO: we could merge adjacent ranges
            ranges,
        }
    }
}

/// A mock encoding verifier.
pub struct MockEncodingVerifier<T>
where
    T: IdSet,
{
    full_encodings: HashMap<Id, [Encoding; 2]>,
    phantom: PhantomData<T>,
}
impl<T> MockEncodingVerifier<T>
where
    T: IdSet,
{
    pub fn new(full_encodings: FullEncodings<T>) -> Self {
        let mut hashmap = HashMap::new();
        for (full_enc, id) in full_encodings
            .encodings
            .iter()
            .zip(full_encodings.ids().ids())
        {
            if hashmap.insert(id.clone(), full_enc.clone()).is_some() {
                panic!("duplicate ids are not allowed");
            }
        }
        Self {
            full_encodings: hashmap,
            phantom: PhantomData,
        }
    }
}
impl<T> EncodingVerifier<T> for MockEncodingVerifier<T>
where
    T: IdSet,
{
    fn init(&self, _init_data: InitData) -> Result<(), EncodingVerifierError> {
        Ok(())
    }

    fn verify(
        &self,
        encodings: ActiveEncodings<T>,
    ) -> Result<FullEncodings<T>, EncodingVerifierError> {
        // Simply return the full encodings corresponding to the active encoding without
        // performing any verification.
        let full_enc = encodings
            .ids()
            .ids()
            .iter()
            .map(|id| self.full_encodings.get(id).unwrap().clone())
            .collect::<Vec<_>>();
        Ok(FullEncodings::new(full_enc, encodings.ids))
    }
}

/// A mock encoding provider.
pub struct MockEncodingProvider<T>
where
    T: IdSet,
{
    /// A mapping from a bit id to the full encodings of the bit.
    full_encodings: HashMap<Id, [Encoding; 2]>,
    phantom: PhantomData<T>,
}
impl<T> MockEncodingProvider<T>
where
    T: IdSet,
{
    pub fn new(full_encodings: FullEncodings<T>) -> Self {
        let mut hashmap = HashMap::new();
        for (full_enc, id) in full_encodings
            .encodings
            .iter()
            .zip(full_encodings.ids().ids())
        {
            if hashmap.insert(id.clone(), full_enc.clone()).is_some() {
                panic!("duplicate ids detected");
            }
        }
        Self {
            full_encodings: hashmap,
            phantom: PhantomData,
        }
    }
}
impl<T> EncodingProvider<T> for MockEncodingProvider<T>
where
    T: IdSet,
{
    fn get_by_ids(&self, ids: T) -> Result<FullEncodings<T>, EncodingProviderError> {
        let full_encodings = ids
            .ids()
            .iter()
            .map(|id| self.full_encodings.get(id).unwrap().clone())
            .collect::<Vec<_>>();
        Ok(FullEncodings::new(full_encodings, ids))
    }
}