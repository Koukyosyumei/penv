use std::io::{self, Cursor};

use super::{
    generated_record::Codec,
    object_stream::{Graph, ObjectId, ObjectReader, ObjectWriter, References, TypeStamp},
    xml_surrogate::{self, XmlSerializable},
};

#[derive(Default)]
pub struct GraphSerializer<'a> {
    support_xml_serializable: bool,
    writer_codecs: Option<&'a [Codec]>,
    reader_codecs: Option<&'a [Codec]>,
}

impl<'a> GraphSerializer<'a> {
    pub fn with_xml_serializable(mut self, enabled: bool) -> Self {
        self.support_xml_serializable = enabled;
        self
    }

    pub fn with_generated_methods(
        mut self,
        writer_codecs: Option<&'a [Codec]>,
        reader_codecs: Option<&'a [Codec]>,
    ) -> Self {
        self.writer_codecs = writer_codecs;
        self.reader_codecs = reader_codecs;
        self
    }

    pub fn clone_graph(
        &self,
        graph: &Graph,
        root: ObjectId,
        stamps: Option<&[TypeStamp<'_>]>,
    ) -> io::Result<(Graph, ObjectId)> {
        let mut bytes = Vec::new();
        let mut writer = ObjectWriter::new(&mut bytes, graph, References::Preserve, true)
            .allow_xml_surrogates(self.support_xml_serializable);
        if let Some(stamps) = stamps {
            writer = writer.with_type_stamps(stamps);
        }
        if let Some(codecs) = self.writer_codecs {
            writer = writer.with_generated_records(codecs);
        }
        writer.write_object(root)?;
        writer.finish()?;

        let mut reader = ObjectReader::new(Cursor::new(bytes), References::Preserve, true)
            .allow_xml_surrogates(self.support_xml_serializable);
        if let Some(stamps) = stamps {
            reader = reader.with_type_stamps(stamps);
        }
        if let Some(codecs) = self.reader_codecs {
            reader = reader.with_generated_records(codecs);
        }
        let root = reader.read_object()?;
        Ok((reader.finish_into_graph()?, root))
    }

    pub fn clone_xml<T: XmlSerializable>(&self, value: &T, stamped: bool) -> io::Result<T> {
        if !self.support_xml_serializable {
            return Err(xml_surrogate::disabled());
        }
        let mut graph = Graph::default();
        let root = xml_surrogate::store(&mut graph, value)?;
        let stamps = [xml_surrogate::TYPE_STAMP];
        let (copy, root) = self.clone_graph(&graph, root, stamped.then_some(&stamps))?;
        xml_surrogate::restore(&copy, root)
    }
}
