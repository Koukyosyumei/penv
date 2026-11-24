//! Statically generated field codecs for the Migrant graph stream.

use super::object_stream::Value;
use std::io;

pub struct Codec {
    pub class: &'static str,
    pub write: fn(&mut dyn RecordOutput, &[Value]) -> io::Result<()>,
    pub read: fn(&mut dyn RecordInput) -> io::Result<Vec<Value>>,
}

pub trait RecordOutput {
    fn count(&mut self, count: usize) -> io::Result<()>;
    fn integer(&mut self, value: &Value) -> io::Result<()>;
    fn float(&mut self, value: &Value) -> io::Result<()>;
    fn reference(&mut self, value: &Value) -> io::Result<()>;
}

pub trait RecordInput {
    fn expect_count(&mut self, count: usize) -> io::Result<()>;
    fn integer(&mut self) -> io::Result<Value>;
    fn float(&mut self) -> io::Result<Value>;
    fn reference(&mut self) -> io::Result<Value>;
}

macro_rules! generated_record {
    ($class:expr; $($field:ident : $kind:ident),* $(,)?) => {
        $crate::migrant::generated_record::Codec {
            class: $class,
            write: |output, fields| {
                let [$($field),*] = fields else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData, "incorrect generated record field count"
                    ));
                };
                output.count(0 $(+ { let _ = $field; 1 })*)?;
                $(output.$kind($field)?;)*
                Ok(())
            },
            read: |input| {
                input.expect_count(0 $(+ { let _ = stringify!($field); 1 })*)?;
                Ok(vec![$(input.$kind()?),*])
            },
        }
    };
}

pub(crate) use generated_record;
