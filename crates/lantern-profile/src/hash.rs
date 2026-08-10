use std::fmt;

use sha2::{Digest, Sha256};

macro_rules! hash_type {
    ($name:ident, $documentation:literal) => {
        #[doc = $documentation]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; 32]);

        impl $name {
            pub(crate) fn digest(bytes: &[u8]) -> Self {
                Self(Sha256::digest(bytes).into())
            }

            /// Returns the binary SHA-256 digest.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }

            /// Returns lowercase hexadecimal text.
            #[must_use]
            pub fn to_hex(self) -> String {
                let mut text = String::with_capacity(64);
                for byte in self.0 {
                    use std::fmt::Write as _;
                    write!(&mut text, "{byte:02x}").expect("writing to String cannot fail");
                }
                text
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                for byte in self.0 {
                    write!(formatter, "{byte:02x}")?;
                }
                Ok(())
            }
        }
    };
}

hash_type!(SourceHash, "SHA-256 of the exact input bytes.");
hash_type!(
    ProfileHash,
    "SHA-256 of the normalized semantic CanonicalProfileV1 model."
);
