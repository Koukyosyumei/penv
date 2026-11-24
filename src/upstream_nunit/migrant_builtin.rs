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
