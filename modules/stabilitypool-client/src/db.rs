use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::impl_db_record;

#[repr(u8)]
#[derive(Clone, Debug)]
pub enum DbKeyPrefix {
    Nonce = 0x01,
}

#[derive(Debug, Clone, Encodable, Decodable, Eq, PartialEq, Hash)]
pub struct NonceKey;

impl_db_record!(key = NonceKey, value = u64, db_prefix = DbKeyPrefix::Nonce,);
