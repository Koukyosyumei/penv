use super::generated_record::Codec;
use super::object_stream::{Graph, Object, ObjectId, ObjectWriter, References, TypeStamp, Value};
use std::{
    collections::HashSet,
    io::{self, Write},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WriteHookPhase {
    Before,
    After,
    Late,
}

pub struct WriteHook<'a> {
    pub class: &'a str,
    pub phase: WriteHookPhase,
    pub apply: fn(&mut Graph, ObjectId) -> io::Result<()>,
}

/// Write a root with callbacks that can update its original records.
pub fn write_hooked(
    stream: impl Write,
    graph: &mut Graph,
    root: ObjectId,
    stamps: Option<&[TypeStamp<'_>]>,
    hooks: &[WriteHook<'_>],
) -> io::Result<()> {
    write_hooked_with_codecs(stream, graph, root, stamps, hooks, None)
}

pub fn write_hooked_with_codecs(
    stream: impl Write,
    graph: &mut Graph,
    root: ObjectId,
    stamps: Option<&[TypeStamp<'_>]>,
    hooks: &[WriteHook<'_>],
    codecs: Option<&[Codec]>,
) -> io::Result<()> {
    let mut visited = HashSet::new();
    let mut order = Vec::new();
    let mut pending = vec![root];
    let before = (|| {
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            order.push(id);
            run(graph, id, WriteHookPhase::Before, hooks)?;
            let fields = match graph.get(id) {
                Some(Object::Record { fields, .. } | Object::Array(fields)) => fields,
                Some(Object::Rectangular { elements, .. }) => elements,
                Some(Object::Bytes(_) | Object::String(_)) => continue,
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid graph reference",
                    ));
                }
            };
            pending.extend(fields.iter().rev().filter_map(|field| match field {
                Value::Ref(child) => Some(*child),
                _ => None,
            }));
        }
        Ok(())
    })();
    let mut result = before.and_then(|()| {
        let mut writer = ObjectWriter::new(stream, graph, References::Preserve, true);
        if let Some(stamps) = stamps {
            writer = writer.with_type_stamps(stamps);
        }
        if let Some(codecs) = codecs {
            writer = writer.with_generated_records(codecs);
        }
        writer.write_object(root)?;
        writer.finish()
    });
    for phase in [WriteHookPhase::After, WriteHookPhase::Late] {
        for &id in order.iter().rev() {
            let callback = run(graph, id, phase, hooks);
            result = result.and(callback);
        }
    }
    result
}

fn run(
    graph: &mut Graph,
    id: ObjectId,
    phase: WriteHookPhase,
    hooks: &[WriteHook<'_>],
) -> io::Result<()> {
    let Some(Object::Record { class, .. }) = graph.get(id) else {
        return Ok(());
    };
    let class = class.clone();
    for hook in hooks
        .iter()
        .filter(|hook| hook.phase == phase && hook.class == class)
    {
        (hook.apply)(graph, id)?;
    }
    Ok(())
}
