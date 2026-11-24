use std::io::{self, Cursor};

use super::{
    object_stream::{Graph, ObjectId, ObjectReader, ObjectWriter, References, TypeStamp},
    xml_surrogate::{self, XmlSerializable},
};

#[derive(Default)]
pub struct ReflectionSerializer {
    support_xml_serializable: bool,
}

impl ReflectionSerializer {
    pub fn with_xml_serializable(mut self, enabled: bool) -> Self {
        self.support_xml_serializable = enabled;
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
        writer.write_object(root)?;
        writer.finish()?;

        let mut reader = ObjectReader::new(Cursor::new(bytes), References::Preserve, true)
            .allow_xml_surrogates(self.support_xml_serializable);
        if let Some(stamps) = stamps {
            reader = reader.with_type_stamps(stamps);
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
