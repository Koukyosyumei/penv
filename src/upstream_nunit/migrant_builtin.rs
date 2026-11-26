use std::io;

use crate::migrant::{
    generated_record::Codec,
    graph_serializer::GraphSerializer,
    object_stream::Graph,
    regex_object,
    xml_surrogate::{self, XmlSerializable},
};

fn serializer(writer: bool, reader: bool, codecs: &'static [Codec]) -> GraphSerializer<'static> {
    GraphSerializer::default()
        .with_xml_serializable(true)
        .with_generated_methods(writer.then_some(codecs), reader.then_some(codecs))
}

fn regex_round_trip(stamped: bool, writer: bool, reader: bool) {
    let regex = regex::Regex::new("[0-9]").unwrap();
    let mut graph = Graph::default();
    let root = regex_object::store(&mut graph, &regex);
    const STAMPS: &[crate::migrant::object_stream::TypeStamp<'static>] =
        &[regex_object::TYPE_STAMP];
    const CODECS: &[Codec] = &[regex_object::GENERATED_CODEC];
    let (graph, root) = serializer(writer, reader, CODECS)
        .clone_graph(&graph, root, stamped.then_some(STAMPS))
        .unwrap();
    let copy = regex_object::restore(&graph, root).unwrap();
    assert_eq!(regex.as_str(), copy.as_str());
}

#[test]
fn regex_plain() {
    regex_round_trip(false, false, false);
}

#[test]
fn regex_stamped() {
    regex_round_trip(true, false, false);
}

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

fn custom_xml(stamped: bool, writer: bool, reader: bool) {
    let data = CustomXml {
        text: "Xavier".into(),
        fake_pointer: 666,
    };
    const CODECS: &[Codec] = &[xml_surrogate::GENERATED_CODEC];
    let copy = serializer(writer, reader, CODECS)
        .clone_xml(&data, stamped)
        .unwrap();
    assert_eq!(copy, data);
}

#[test]
fn custom_xml_plain() {
    custom_xml(false, false, false);
}

#[test]
fn custom_xml_stamped() {
    custom_xml(true, false, false);
}

macro_rules! modes {
    ($w:literal, $r:literal, $s:literal; $regex:ident, $xml:ident) => {
        #[test]
        fn $regex() {
            regex_round_trip($s, $w, $r);
        }

        #[test]
        fn $xml() {
            custom_xml($s, $w, $r);
        }
    };
}
modes!(false, true, false; regex_generated_reader_plain, xml_generated_reader_plain);
modes!(false, true, true; regex_generated_reader_stamped, xml_generated_reader_stamped);
modes!(true, false, false; regex_generated_writer_plain, xml_generated_writer_plain);
modes!(true, false, true; regex_generated_writer_stamped, xml_generated_writer_stamped);
modes!(true, true, false; regex_generated_both_plain, xml_generated_both_plain);
modes!(true, true, true; regex_generated_both_stamped, xml_generated_both_stamped);
