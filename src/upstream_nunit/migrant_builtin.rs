use crate::migrant::{
    object_stream::{Graph, ObjectReader, ObjectWriter, References},
    regex_object,
};
use std::io::{Cursor, Seek};

fn regex_round_trip(stamped: bool) {
    let regex = regex::Regex::new("[0-9]").unwrap();
    let mut graph = Graph::default();
    let root = regex_object::store(&mut graph, &regex);
    let mut stream = Cursor::new(Vec::new());
    const STAMPS: &[crate::migrant::object_stream::TypeStamp<'static>] =
        &[regex_object::TYPE_STAMP];

    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.rewind().unwrap();

    let mut reader = ObjectReader::new(stream, References::Preserve, true);
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    let root = reader.read_object().unwrap();
    let copy = regex_object::restore(reader.graph(), root).unwrap();
    assert_eq!(regex.as_str(), copy.as_str());
    reader.finish().unwrap();
}

#[test]
fn regex_plain() {
    regex_round_trip(false);
}

#[test]
fn regex_stamped() {
    regex_round_trip(true);
}

use crate::migrant::xml_surrogate::{self, XmlSerializable};
use std::io;

#[derive(Debug, PartialEq, Eq)]
struct CustomXml {
    text: String,
    fake_pointer: i32,
}

impl XmlSerializable for CustomXml {
    const TYPE_NAME: &'static str = "Antmicro.Migrant.Tests.CustomIXmlSerializable, Tests";

    fn write_xml(&self) -> io::Result<Vec<u8>> {
        let text = self
            .text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        Ok(format!(
            "<CustomIXmlSerializable><S>{text}</S><I>{}</I></CustomIXmlSerializable>",
            self.fake_pointer
        )
        .into_bytes())
    }

    fn read_xml(bytes: &[u8]) -> io::Result<Self> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid custom XML");
        let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
        let (text, number) = text
            .strip_prefix("<CustomIXmlSerializable><S>")
            .and_then(|text| text.strip_suffix("</I></CustomIXmlSerializable>"))
            .and_then(|text| text.split_once("</S><I>"))
            .ok_or_else(invalid)?;
        Ok(Self {
            text: text
                .replace("&gt;", ">")
                .replace("&lt;", "<")
                .replace("&amp;", "&"),
            fake_pointer: number.parse().map_err(io::Error::other)?,
        })
    }
}

fn custom_xml(stamped: bool) {
    let data = CustomXml {
        text: "Xavier".into(),
        fake_pointer: 666,
    };
    let mut graph = Graph::default();
    let root = xml_surrogate::store(&mut graph, &data).unwrap();
    let mut stream = Cursor::new(Vec::new());
    const STAMPS: &[crate::migrant::object_stream::TypeStamp<'static>] =
        &[xml_surrogate::TYPE_STAMP];
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.rewind().unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true);
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    let id = reader.read_object().unwrap();
    assert_eq!(
        xml_surrogate::restore::<CustomXml>(reader.graph(), id).unwrap(),
        data
    );
    reader.finish().unwrap();
}

#[test]
fn custom_xml_plain() {
    custom_xml(false);
}

#[test]
fn custom_xml_stamped() {
    custom_xml(true);
}
