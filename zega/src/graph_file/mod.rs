//! `.graph`: one file to export a graph from any zega surface and import it
//! into any other. The format is specified in `docs/graph-format.md`; this
//! module is its reference implementation. Keep the two in step.
//!
//! - The writer streams: two passes over the graph per section (one to size
//!   and checksum it, one to write it), never a copy of the graph or of the
//!   file. Its extra memory is the name dictionary and a write buffer.
//! - The reader streams: it decodes record by record into a new [`Graph`]
//!   and never holds a section or the file. The caller installs that graph
//!   only once the whole file, down to the final digest, has been checked,
//!   which is what makes an import all-or-nothing.
//! - One graph has exactly one encoding: every map is written in key order,
//!   every id list in ascending order, and the reader rejects anything else.
//!   So the same graph gives the same bytes, and an accepted file exports
//!   back to itself.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{self, BufReader, BufWriter, Read, Write};

use sha2::{Digest, Sha256};

use crate::graph::{Graph, Node, NodeId, Relationship};
use crate::index::{IndexKind, IndexSpec};
use crate::location::Point;
use crate::value::Value;
use crate::vector::{Metric, Vector};

#[cfg(test)]
mod tests;

/// The first eight bytes of every `.graph` file. The high first byte and the
/// newline catch a file that went through a text-mode transfer.
pub const MAGIC: [u8; 8] = *b"\x89ZGRAPH\n";

/// The format version this build writes, and the newest it reads. It counts
/// format changes only; it is independent of the engine version.
pub const FORMAT_VERSION: u32 = 1;

/// The media type `zega start` serves `GET /graph` with.
pub const MEDIA_TYPE: &str = "application/vnd.zega.graph";

/// The deepest nesting of lists and maps inside one property value: ZQL's
/// own nesting limit, and deeper than any JSON load can produce.
pub const MAX_VALUE_DEPTH: usize = 128;

const BUFFER: usize = 64 * 1024;

/// The most a reader reserves up front for a count the file claims. A count
/// is only a claim until its items arrive, so collections grow with the
/// bytes actually read: a few bytes can never make the reader allocate much.
const RESERVE: usize = 16;

/// The sections of a version 1 file, in the order they must appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Manifest,
    Names,
    Schema,
    Nodes,
    Relationships,
    Done,
}

impl Section {
    pub fn tag(self) -> [u8; 4] {
        *match self {
            Section::Manifest => b"MNFT",
            Section::Names => b"NAME",
            Section::Schema => b"SCHM",
            Section::Nodes => b"NODE",
            Section::Relationships => b"RELS",
            Section::Done => b"DONE",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Section::Manifest => "manifest",
            Section::Names => "names",
            Section::Schema => "schema",
            Section::Nodes => "nodes",
            Section::Relationships => "relationships",
            Section::Done => "done",
        }
    }
}

/// Why a `.graph` file could not be written or read. Every read error means
/// nothing was imported.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a .graph file: it does not start with the .graph magic bytes")]
    BadMagic,
    #[error(
        "this file is .graph format version {found}; this zega reads versions 1 to {supported}. \
         Upgrade zega to import it"
    )]
    NewerVersion { found: u32, supported: u32 },
    #[error("truncated .graph file: it ends at byte {offset}, {context}")]
    Truncated { offset: u64, context: String },
    #[error(
        "corrupt .graph file: the {section} section's checksum is {actual:#010x}, \
         the file says {expected:#010x}"
    )]
    Checksum {
        section: &'static str,
        expected: u32,
        actual: u32,
    },
    #[error("invalid .graph file: {reason} (byte {offset}, {section} section)")]
    Invalid {
        section: &'static str,
        offset: u64,
        reason: String,
    },
    #[error("cannot export this graph as .graph: {0}")]
    Unexportable(String),
    #[error(".graph i/o error: {0}")]
    Io(#[from] io::Error),
}

/// What an export carries besides the graph itself. Anything given here
/// overrides what the graph carries from its last import.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// ZQL schema text to travel with the graph (types, `unique`, `index`).
    /// `None` keeps the schema the graph was imported with, if any.
    pub schema: Option<String>,
    /// Manifest metadata, merged over what the graph was imported with.
    /// Well-known keys: `title`, `source`, `source_version`, `licence`,
    /// `fetched_at` (docs/graph-format.md).
    pub meta: BTreeMap<String, String>,
}

/// What a `.graph` file carries besides nodes and relationships: its schema
/// text, the declarations that schema makes, and its manifest metadata. A
/// graph keeps these from its import, in memory, in the WAL (through the
/// imported file) and in snapshots, and exports them again, so import then
/// export gives back the same file.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Carried {
    pub schema: Option<String>,
    pub uniques: Vec<(String, String)>,
    pub indexes: Vec<IndexSpec>,
    pub meta: BTreeMap<String, String>,
}

/// What an export wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSummary {
    pub nodes: u64,
    pub relationships: u64,
    pub bytes: u64,
    /// SHA-256 of the graph's content sections, as hex: equal for equal
    /// graphs whatever wrote them (docs/graph-format.md, "Content digest").
    pub content_sha256: String,
}

/// An index declaration as a file carries it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct IndexDeclaration {
    pub kind: &'static str,
    pub type_name: String,
    pub field: String,
}

/// What an import read, besides the graph it installed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ImportSummary {
    pub format_version: u32,
    pub created_by: String,
    pub nodes: u64,
    pub relationships: u64,
    pub schema: Option<String>,
    pub uniques: Vec<(String, String)>,
    pub indexes: Vec<IndexDeclaration>,
    pub meta: BTreeMap<String, String>,
    pub content_sha256: String,
}

// ---------------------------------------------------------------------------
// Writing

/// Where section bytes go: [`Measure`] on the sizing pass, [`Emit`] on the
/// writing pass. Both passes run the same encoder, so the length and checksum
/// in the section header describe exactly the bytes that follow.
trait Sink {
    fn put(&mut self, bytes: &[u8]) -> io::Result<()>;
}

#[derive(Default)]
struct Measure {
    len: u64,
    crc: crc32fast::Hasher,
}

impl Sink for Measure {
    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.len += bytes.len() as u64;
        self.crc.update(bytes);
        Ok(())
    }
}

struct Out<W: Write> {
    inner: BufWriter<W>,
    bytes: u64,
    content: Option<Sha256>,
}

impl<W: Write> Out<W> {
    fn raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_all(bytes)?;
        self.bytes += bytes.len() as u64;
        if let Some(content) = &mut self.content {
            content.update(bytes);
        }
        Ok(())
    }
}

struct Emit<'a, W: Write> {
    out: &'a mut Out<W>,
    len: u64,
}

impl<W: Write> Sink for Emit<'_, W> {
    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.len += bytes.len() as u64;
        self.out.raw(bytes)
    }
}

type Encoder<'a> = dyn Fn(&mut dyn Sink) -> Result<(), Error> + 'a;

fn put_u8(sink: &mut dyn Sink, value: u8) -> Result<(), Error> {
    Ok(sink.put(&[value])?)
}

fn put_u32(sink: &mut dyn Sink, value: u32) -> Result<(), Error> {
    Ok(sink.put(&value.to_le_bytes())?)
}

fn put_u64(sink: &mut dyn Sink, value: u64) -> Result<(), Error> {
    Ok(sink.put(&value.to_le_bytes())?)
}

fn put_len(sink: &mut dyn Sink, len: usize, what: &str) -> Result<(), Error> {
    let len = u32::try_from(len)
        .map_err(|_| Error::Unexportable(format!("{what} has more than 2^32-1 entries or bytes")))?;
    put_u32(sink, len)
}

fn put_str(sink: &mut dyn Sink, text: &str) -> Result<(), Error> {
    put_len(sink, text.len(), "a string")?;
    Ok(sink.put(text.as_bytes())?)
}

fn section<W: Write>(out: &mut Out<W>, section: Section, encode: &Encoder<'_>) -> Result<(), Error> {
    let mut measure = Measure::default();
    encode(&mut measure)?;
    out.raw(&section.tag())?;
    out.raw(&measure.len.to_le_bytes())?;
    let mut emit = Emit { out, len: 0 };
    encode(&mut emit)?;
    if emit.len != measure.len {
        return Err(Error::Unexportable(format!(
            "the {} section changed size between passes ({} then {} bytes)",
            section.name(),
            measure.len,
            emit.len
        )));
    }
    out.raw(&measure.crc.finalize().to_le_bytes())?;
    Ok(())
}

/// Every label, relationship kind and top-level property key, sorted by
/// bytes; a name's position is its id.
struct Names<'g> {
    sorted: Vec<&'g str>,
    ids: HashMap<&'g str, u32>,
}

impl<'g> Names<'g> {
    fn collect(graph: &'g Graph) -> Result<Self, Error> {
        let mut set: BTreeSet<&'g str> = BTreeSet::new();
        for node in graph.all_nodes().values() {
            set.extend(node.labels.iter().map(String::as_str));
            set.extend(node.props.keys().map(String::as_str));
        }
        for rel in graph.all_relationships().values() {
            set.insert(rel.kind.as_str());
            set.extend(rel.props.keys().map(String::as_str));
        }
        let sorted: Vec<&str> = set.into_iter().collect();
        if u32::try_from(sorted.len()).is_err() {
            return Err(Error::Unexportable("more than 2^32-1 distinct names".into()));
        }
        let ids = sorted.iter().enumerate().map(|(id, name)| (*name, id as u32)).collect();
        Ok(Names { sorted, ids })
    }

    fn id(&self, name: &str) -> u32 {
        self.ids[name]
    }
}

/// The values of `map` in ascending id order, without sorting a copy of the
/// ids when they are dense (the usual case): walk `0..=max` and look each up.
fn ascending<T>(map: &HashMap<u64, T>) -> Box<dyn Iterator<Item = &T> + '_> {
    let Some(&max) = map.keys().max() else {
        return Box::new(std::iter::empty());
    };
    let dense = max <= (map.len() as u64).saturating_mul(2).saturating_add(1024);
    if dense {
        Box::new((0..=max).filter_map(move |id| map.get(&id)))
    } else {
        let mut ids: Vec<u64> = map.keys().copied().collect();
        ids.sort_unstable();
        Box::new(ids.into_iter().map(move |id| &map[&id]))
    }
}

fn put_value(sink: &mut dyn Sink, value: &Value, depth: usize) -> Result<(), Error> {
    match value {
        Value::Null => put_u8(sink, tag::NULL),
        Value::Bool(false) => put_u8(sink, tag::FALSE),
        Value::Bool(true) => put_u8(sink, tag::TRUE),
        Value::Int(int) => {
            put_u8(sink, tag::INT)?;
            Ok(sink.put(&int.to_le_bytes())?)
        }
        Value::Float(bits) => {
            put_u8(sink, tag::FLOAT)?;
            put_u64(sink, *bits)
        }
        Value::String(text) => {
            put_u8(sink, tag::STRING)?;
            put_str(sink, text)
        }
        Value::List(items) => {
            nested(depth)?;
            put_u8(sink, tag::LIST)?;
            put_len(sink, items.len(), "a list")?;
            for item in items {
                put_value(sink, item, depth + 1)?;
            }
            Ok(())
        }
        Value::Map(map) => {
            nested(depth)?;
            put_u8(sink, tag::MAP)?;
            put_len(sink, map.len(), "a map")?;
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            for (key, item) in entries {
                put_str(sink, key)?;
                put_value(sink, item, depth + 1)?;
            }
            Ok(())
        }
        Value::Point(point) => {
            put_u8(sink, tag::POINT)?;
            put_u64(sink, point.lat().to_bits())?;
            put_u64(sink, point.lon().to_bits())
        }
        Value::Vector(vector) => {
            put_u8(sink, tag::VECTOR)?;
            put_u8(sink, metric_code(vector.metric))?;
            put_len(sink, vector.dimensions(), "a vector")?;
            for bits in vector.bits() {
                put_u32(sink, *bits)?;
            }
            Ok(())
        }
    }
}

fn nested(depth: usize) -> Result<(), Error> {
    if depth >= MAX_VALUE_DEPTH {
        return Err(Error::Unexportable(format!(
            "a property value nests lists and maps more than {MAX_VALUE_DEPTH} deep"
        )));
    }
    Ok(())
}

fn put_props(
    sink: &mut dyn Sink,
    names: &Names<'_>,
    props: &HashMap<String, Value>,
) -> Result<(), Error> {
    put_len(sink, props.len(), "a property map")?;
    let mut entries: Vec<(&String, &Value)> = props.iter().collect();
    entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
    for (key, value) in entries {
        put_u32(sink, names.id(key))?;
        put_value(sink, value, 0)?;
    }
    Ok(())
}

fn put_node(sink: &mut dyn Sink, names: &Names<'_>, node: &Node) -> Result<(), Error> {
    if let Some(label) = duplicate_label(&node.labels) {
        return Err(Error::Unexportable(format!(
            "node {} has the label {label:?} twice",
            node.id
        )));
    }
    put_u64(sink, node.id)?;
    put_len(sink, node.labels.len(), "a label list")?;
    for label in &node.labels {
        put_u32(sink, names.id(label))?;
    }
    put_props(sink, names, &node.props)
}

fn put_relationship(
    sink: &mut dyn Sink,
    graph: &Graph,
    names: &Names<'_>,
    rel: &Relationship,
) -> Result<(), Error> {
    for end in [rel.from, rel.to] {
        if graph.get_node(end).is_none() {
            return Err(Error::Unexportable(format!(
                "relationship {} points at node {end}, which does not exist",
                rel.id
            )));
        }
    }
    put_u64(sink, rel.id)?;
    put_u32(sink, names.id(&rel.kind))?;
    put_u64(sink, rel.from)?;
    put_u64(sink, rel.to)?;
    put_props(sink, names, &rel.props)
}

/// The first label that appears twice in `labels`.
fn duplicate_label(labels: &[String]) -> Option<&String> {
    if labels.len() <= 8 {
        return labels.iter().enumerate().find(|(i, l)| labels[..*i].contains(l)).map(|(_, l)| l);
    }
    let mut seen = std::collections::HashSet::new();
    labels.iter().find(|label| !seen.insert(label.as_str()))
}

/// `unique` `(type, field)` pairs and index declarations.
type Declarations = (Vec<(String, String)>, Vec<IndexSpec>);

/// The `unique` fields and the indexes a schema declares, sorted as the file
/// stores them. The indexes are the engine's own reading of the schema
/// (`index` blocks plus the range index every orderable `unique` field
/// gets), so a file declares exactly what running that schema would.
fn declarations(source: &str) -> Result<Declarations, Error> {
    let unparsable = |error: crate::lang::Error| {
        Error::Unexportable(format!("the schema does not parse: {error}"))
    };
    let schema = crate::lang::parse_schema(source).map_err(unparsable)?;
    let mut uniques = crate::lang::parse_uniques(source).map_err(unparsable)?;
    let indexes = crate::lang::parse_indexes(source).map_err(unparsable)?;
    let mut indexes = crate::lang::effective_indexes(&schema, &uniques, &indexes);
    uniques.sort();
    uniques.dedup();
    indexes.sort_by(|a, b| (&a.type_name, &a.field, a.kind).cmp(&(&b.type_name, &b.field, b.kind)));
    indexes.dedup();
    Ok((uniques, indexes))
}

/// Write `graph` as a `.graph` file to `out`. `created_by` names the writer
/// in the manifest (`zega 0.2.0`).
pub(crate) fn write<W: Write>(
    graph: &Graph,
    options: &ExportOptions,
    created_by: &str,
    out: W,
) -> Result<ExportSummary, Error> {
    let names = Names::collect(graph)?;
    let carried = graph.carried();
    let (schema, uniques, indexes) = match &options.schema {
        Some(source) => {
            let (uniques, indexes) = declarations(source)?;
            (Some(source), uniques, indexes)
        }
        None => (carried.schema.as_ref(), carried.uniques.clone(), carried.indexes.clone()),
    };
    let mut meta = carried.meta.clone();
    meta.extend(options.meta.iter().map(|(k, v)| (k.clone(), v.clone())));
    let (next_node, next_rel) = graph.next_ids();
    let node_count = graph.all_nodes().len() as u64;
    let rel_count = graph.all_relationships().len() as u64;

    let mut out = Out {
        inner: BufWriter::with_capacity(BUFFER, out),
        bytes: 0,
        content: None,
    };
    out.raw(&MAGIC)?;
    out.raw(&FORMAT_VERSION.to_le_bytes())?;
    section(&mut out, Section::Manifest, &|sink| {
        put_str(sink, created_by)?;
        put_u64(sink, node_count)?;
        put_u64(sink, rel_count)?;
        put_len(sink, meta.len(), "the manifest metadata")?;
        for (key, value) in &meta {
            put_str(sink, key)?;
            put_str(sink, value)?;
        }
        Ok(())
    })?;
    out.content = Some(Sha256::new());
    section(&mut out, Section::Names, &|sink| {
        put_len(sink, names.sorted.len(), "the name dictionary")?;
        for name in &names.sorted {
            put_str(sink, name)?;
        }
        Ok(())
    })?;
    section(&mut out, Section::Schema, &|sink| {
        match schema {
            Some(source) => {
                put_u8(sink, 1)?;
                put_str(sink, source)?;
            }
            None => put_u8(sink, 0)?,
        }
        put_len(sink, indexes.len(), "the index list")?;
        for spec in &indexes {
            put_u8(sink, index_code(spec.kind))?;
            put_str(sink, &spec.type_name)?;
            put_str(sink, &spec.field)?;
        }
        put_len(sink, uniques.len(), "the unique list")?;
        for (type_name, field) in &uniques {
            put_str(sink, type_name)?;
            put_str(sink, field)?;
        }
        Ok(())
    })?;
    section(&mut out, Section::Nodes, &|sink| {
        put_u64(sink, next_node)?;
        for node in ascending(graph.all_nodes()) {
            put_node(sink, &names, node)?;
        }
        Ok(())
    })?;
    section(&mut out, Section::Relationships, &|sink| {
        put_u64(sink, next_rel)?;
        for rel in ascending(graph.all_relationships()) {
            put_relationship(sink, graph, &names, rel)?;
        }
        Ok(())
    })?;
    let digest: [u8; 32] = out
        .content
        .take()
        .map(|content| content.finalize().into())
        .unwrap_or_default();
    section(&mut out, Section::Done, &|sink| Ok(sink.put(&digest)?))?;
    out.inner.flush()?;
    Ok(ExportSummary {
        nodes: node_count,
        relationships: rel_count,
        bytes: out.bytes,
        content_sha256: hex(&digest),
    })
}

// ---------------------------------------------------------------------------
// Reading

struct In<R: Read> {
    inner: R,
    offset: u64,
    content: Option<Sha256>,
}

impl<R: Read> In<R> {
    /// Fill `buf`, or return how many bytes arrived before the end of input.
    fn fill(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let mut got = 0;
        while got < buf.len() {
            match self.inner.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.offset += got as u64;
        if let Some(content) = &mut self.content {
            content.update(&buf[..got]);
        }
        Ok(got)
    }

    fn exact(&mut self, buf: &mut [u8], context: impl FnOnce() -> String) -> Result<(), Error> {
        let got = self.fill(buf)?;
        if got < buf.len() {
            return Err(Error::Truncated {
                offset: self.offset,
                context: context(),
            });
        }
        Ok(())
    }
}

/// Reads one section's payload: never past its declared length, feeding its
/// checksum as it goes.
struct SectionIn<'a, R: Read> {
    input: &'a mut In<R>,
    section: Section,
    remaining: u64,
    crc: crc32fast::Hasher,
}

impl<R: Read> SectionIn<'_, R> {
    fn invalid(&self, reason: impl Into<String>) -> Error {
        Error::Invalid {
            section: self.section.name(),
            offset: self.input.offset,
            reason: reason.into(),
        }
    }

    fn truncated(&self) -> String {
        format!("inside the {} section", self.section.name())
    }

    fn take(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        if (buf.len() as u64) > self.remaining {
            return Err(self.invalid(format!(
                "a record runs past the end of the section ({} bytes needed, {} left)",
                buf.len(),
                self.remaining
            )));
        }
        let context = self.truncated();
        self.input.exact(buf, || context)?;
        self.remaining -= buf.len() as u64;
        self.crc.update(buf);
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, Error> {
        let mut buf = [0u8; 1];
        self.take(&mut buf)?;
        Ok(buf[0])
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let mut buf = [0u8; 4];
        self.take(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let mut buf = [0u8; 8];
        self.take(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    /// A count of items at least `min_size` bytes each: one that cannot fit
    /// in what is left of the section is refused before anything is
    /// allocated for it.
    fn count(&mut self, min_size: u64) -> Result<usize, Error> {
        let count = self.u32()?;
        if u64::from(count).saturating_mul(min_size) > self.remaining {
            return Err(self.invalid(format!(
                "a count of {count} cannot fit in the {} bytes left in the section",
                self.remaining
            )));
        }
        Ok(count as usize)
    }

    fn bytes(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        if len as u64 > self.remaining {
            return Err(self.invalid(format!(
                "a string of {len} bytes runs past the end of the section"
            )));
        }
        // Grows as bytes arrive rather than trusting `len` up front.
        let mut bytes = Vec::with_capacity(len.min(4096));
        let mut chunk = [0u8; 4096];
        while bytes.len() < len {
            let n = (len - bytes.len()).min(chunk.len());
            self.take(&mut chunk[..n])?;
            bytes.extend_from_slice(&chunk[..n]);
        }
        Ok(bytes)
    }

    fn string(&mut self) -> Result<String, Error> {
        let len = self.u32()? as usize;
        let bytes = self.bytes(len)?;
        String::from_utf8(bytes).map_err(|_| self.invalid("a string is not valid UTF-8"))
    }

    /// Read and discard the rest of the section, still checksumming it.
    fn drain(&mut self) -> Result<(), Error> {
        let mut chunk = [0u8; 4096];
        while self.remaining > 0 {
            let n = self.remaining.min(chunk.len() as u64) as usize;
            self.take(&mut chunk[..n])?;
        }
        Ok(())
    }
}

/// Read one section: its header, its payload through `decode`, and its
/// checksum. A payload that fails to decode is drained and checksummed first,
/// so damage in transit is reported as a checksum error rather than as
/// whatever the damaged bytes happened to look like.
fn read_section<R: Read, T>(
    input: &mut In<R>,
    section: Section,
    decode: impl FnOnce(&mut SectionIn<'_, R>) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut header = [0u8; 12];
    let start = input.offset;
    let got = input.fill(&mut header)?;
    if got < header.len() {
        return Err(Error::Truncated {
            offset: input.offset,
            context: if got == 0 {
                format!("where the {} section should start", section.name())
            } else {
                format!("inside the {} section header", section.name())
            },
        });
    }
    if header[..4] != section.tag() {
        return Err(Error::Invalid {
            section: section.name(),
            offset: start,
            reason: format!(
                "expected the {} section (tag {:?}), found tag {:?}",
                section.name(),
                String::from_utf8_lossy(&section.tag()),
                String::from_utf8_lossy(&header[..4])
            ),
        });
    }
    let len = u64::from_le_bytes(header[4..].try_into().expect("8 bytes"));
    let mut payload = SectionIn {
        input,
        section,
        remaining: len,
        crc: crc32fast::Hasher::new(),
    };
    let decoded = match decode(&mut payload) {
        Ok(value) if payload.remaining == 0 => Ok(value),
        Ok(_) => Err(payload.invalid(format!(
            "{} bytes left over after the section's last record",
            payload.remaining
        ))),
        Err(error @ (Error::Truncated { .. } | Error::Io(_))) => return Err(error),
        Err(error) => Err(error),
    };
    if decoded.is_err() {
        payload.drain()?;
    }
    let actual = payload.crc.finalize();
    let mut crc = [0u8; 4];
    input.exact(&mut crc, || format!("inside the {} section's checksum", section.name()))?;
    let expected = u32::from_le_bytes(crc);
    if expected != actual {
        return Err(Error::Checksum {
            section: section.name(),
            expected,
            actual,
        });
    }
    decoded
}

struct Manifest {
    created_by: String,
    nodes: u64,
    relationships: u64,
    meta: BTreeMap<String, String>,
}

fn read_manifest<R: Read>(s: &mut SectionIn<'_, R>) -> Result<Manifest, Error> {
    let created_by = s.string()?;
    let nodes = s.u64()?;
    let relationships = s.u64()?;
    let count = s.count(8)?;
    let mut meta = BTreeMap::new();
    let mut last: Option<String> = None;
    for _ in 0..count {
        let key = s.string()?;
        let value = s.string()?;
        if last.as_ref().is_some_and(|last| *last >= key) {
            return Err(s.invalid("manifest metadata keys must be unique and in ascending order"));
        }
        last = Some(key.clone());
        meta.insert(key, value);
    }
    Ok(Manifest {
        created_by,
        nodes,
        relationships,
        meta,
    })
}

fn read_names<R: Read>(s: &mut SectionIn<'_, R>) -> Result<Vec<String>, Error> {
    let count = s.count(4)?;
    let mut names: Vec<String> = Vec::with_capacity(count.min(RESERVE));
    for _ in 0..count {
        let name = s.string()?;
        if names.last().is_some_and(|last| *last >= name) {
            return Err(s.invalid("names must be unique and in ascending byte order"));
        }
        names.push(name);
    }
    Ok(names)
}

struct Schema {
    source: Option<String>,
    indexes: Vec<IndexSpec>,
    uniques: Vec<(String, String)>,
}

fn read_schema<R: Read>(s: &mut SectionIn<'_, R>) -> Result<Schema, Error> {
    let source = match s.u8()? {
        0 => None,
        1 => Some(s.string()?),
        other => return Err(s.invalid(format!("schema presence flag must be 0 or 1, not {other}"))),
    };
    let count = s.count(9)?;
    let mut indexes: Vec<IndexSpec> = Vec::with_capacity(count.min(RESERVE));
    for _ in 0..count {
        let kind = match s.u8()? {
            0 => IndexKind::Range,
            1 => IndexKind::Text,
            other => return Err(s.invalid(format!("unknown index kind {other}"))),
        };
        let spec = IndexSpec {
            kind,
            type_name: s.string()?,
            field: s.string()?,
        };
        let key = |spec: &IndexSpec| (spec.type_name.clone(), spec.field.clone(), spec.kind);
        if indexes.last().is_some_and(|last| key(last) >= key(&spec)) {
            return Err(s.invalid("index declarations must be unique and sorted by type, field, kind"));
        }
        indexes.push(spec);
    }
    let count = s.count(8)?;
    let mut uniques: Vec<(String, String)> = Vec::with_capacity(count.min(RESERVE));
    for _ in 0..count {
        let unique = (s.string()?, s.string()?);
        if uniques.last().is_some_and(|last| *last >= unique) {
            return Err(s.invalid("unique declarations must be unique and sorted by type, field"));
        }
        uniques.push(unique);
    }
    if source.is_none() && !(indexes.is_empty() && uniques.is_empty()) {
        return Err(s.invalid(
            "index and unique declarations need the schema text they come from",
        ));
    }
    Ok(Schema {
        source,
        indexes,
        uniques,
    })
}

/// Name ids resolved against the dictionary, remembering which were used:
/// a name nothing refers to is an error, so the dictionary is exactly the
/// one the writer would build.
struct NameTable {
    names: Vec<String>,
    used: Vec<bool>,
}

impl NameTable {
    fn get<R: Read>(&mut self, s: &SectionIn<'_, R>, id: u32) -> Result<String, Error> {
        let index = id as usize;
        let name = self
            .names
            .get(index)
            .ok_or_else(|| s.invalid(format!("name id {id} is not in the name dictionary")))?;
        self.used[index] = true;
        Ok(name.clone())
    }
}

/// One value. Lists and maps recurse through [`read_list`] and [`read_map`];
/// everything else is read by non-inlined helpers, so each level of nesting
/// costs two small stack frames (the depth limit must hold on wasm's 1 MiB
/// stack, in a debug build too).
fn read_value<R: Read>(s: &mut SectionIn<'_, R>, depth: usize) -> Result<Value, Error> {
    match s.u8()? {
        tag::LIST | tag::MAP if depth >= MAX_VALUE_DEPTH => Err(s.invalid(format!(
            "a value nests lists and maps more than {MAX_VALUE_DEPTH} deep"
        ))),
        tag::LIST => read_list(s, depth),
        tag::MAP => read_map(s, depth),
        code => read_scalar(s, code),
    }
}

#[inline(never)]
fn read_list<R: Read>(s: &mut SectionIn<'_, R>, depth: usize) -> Result<Value, Error> {
    let count = s.count(1)?;
    let mut items = Vec::with_capacity(count.min(RESERVE));
    for _ in 0..count {
        items.push(read_value(s, depth + 1)?);
    }
    Ok(Value::List(items))
}

#[inline(never)]
fn read_map<R: Read>(s: &mut SectionIn<'_, R>, depth: usize) -> Result<Value, Error> {
    let count = s.count(5)?;
    let mut map = HashMap::with_capacity(count.min(RESERVE));
    let mut last: Option<String> = None;
    for _ in 0..count {
        let key = s.string()?;
        if last.as_ref().is_some_and(|last| *last >= key) {
            return Err(s.invalid("map keys must be unique and in ascending byte order"));
        }
        let value = read_value(s, depth + 1)?;
        last = Some(key.clone());
        map.insert(key, value);
    }
    Ok(Value::Map(map))
}

#[inline(never)]
fn read_scalar<R: Read>(s: &mut SectionIn<'_, R>, code: u8) -> Result<Value, Error> {
    Ok(match code {
        tag::NULL => Value::Null,
        tag::FALSE => Value::Bool(false),
        tag::TRUE => Value::Bool(true),
        tag::INT => Value::Int(s.u64()? as i64),
        tag::FLOAT => Value::Float(s.u64()?),
        tag::STRING => Value::String(s.string()?),
        tag::POINT => {
            let (lat, lon) = (s.u64()?, s.u64()?);
            let point = Point::new(f64::from_bits(lat), f64::from_bits(lon))
                .map_err(|reason| s.invalid(reason))?;
            if point.lat().to_bits() != lat || point.lon().to_bits() != lon {
                return Err(s.invalid("a point coordinate -0.0 must be stored as 0.0"));
            }
            Value::Point(point)
        }
        tag::VECTOR => {
            let metric = match s.u8()? {
                0 => Metric::Cosine,
                1 => Metric::Dot,
                2 => Metric::L2,
                other => return Err(s.invalid(format!("unknown vector metric {other}"))),
            };
            let dimensions = s.count(4)?;
            let mut bits = Vec::with_capacity(dimensions.min(RESERVE));
            for _ in 0..dimensions {
                bits.push(s.u32()?);
            }
            Value::Vector(Vector::from_bits(bits, metric).map_err(|reason| s.invalid(reason))?)
        }
        other => return Err(s.invalid(format!("unknown value tag {other:#04x}"))),
    })
}

fn read_props<R: Read>(
    s: &mut SectionIn<'_, R>,
    names: &mut NameTable,
) -> Result<HashMap<String, Value>, Error> {
    let count = s.count(5)?;
    let mut props = HashMap::with_capacity(count.min(RESERVE));
    let mut last: Option<u32> = None;
    for _ in 0..count {
        let key = s.u32()?;
        if last.is_some_and(|last| last >= key) {
            return Err(s.invalid("property keys must be unique and in ascending order"));
        }
        last = Some(key);
        let key = names.get(s, key)?;
        props.insert(key, read_value(s, 0)?);
    }
    Ok(props)
}

fn read_nodes<R: Read>(
    s: &mut SectionIn<'_, R>,
    graph: &mut Graph,
    names: &mut NameTable,
    manifest: &Manifest,
) -> Result<u64, Error> {
    let next_node = s.u64()?;
    let mut last: Option<NodeId> = None;
    for _ in 0..manifest.nodes {
        let id = s.u64()?;
        if last.is_some_and(|last| last >= id) {
            return Err(s.invalid(format!("node {id} is out of order: node ids must ascend")));
        }
        if id >= next_node {
            return Err(s.invalid(format!(
                "node {id} is not below the next node id {next_node}"
            )));
        }
        last = Some(id);
        let count = s.count(4)?;
        let mut labels = Vec::with_capacity(count.min(RESERVE));
        for _ in 0..count {
            let label = s.u32()?;
            labels.push(names.get(s, label)?);
        }
        if let Some(label) = duplicate_label(&labels) {
            return Err(s.invalid(format!("node {id} has the label {label:?} twice")));
        }
        let props = read_props(s, names)?;
        graph.restore_node(id, labels, props);
    }
    Ok(next_node)
}

fn read_relationships<R: Read>(
    s: &mut SectionIn<'_, R>,
    graph: &mut Graph,
    names: &mut NameTable,
    manifest: &Manifest,
) -> Result<u64, Error> {
    let next_rel = s.u64()?;
    let mut last: Option<u64> = None;
    for _ in 0..manifest.relationships {
        let id = s.u64()?;
        if last.is_some_and(|last| last >= id) {
            return Err(s.invalid(format!(
                "relationship {id} is out of order: relationship ids must ascend"
            )));
        }
        if id >= next_rel {
            return Err(s.invalid(format!(
                "relationship {id} is not below the next relationship id {next_rel}"
            )));
        }
        last = Some(id);
        let kind = s.u32()?;
        let kind = names.get(s, kind)?;
        let (from, to) = (s.u64()?, s.u64()?);
        for end in [from, to] {
            if graph.get_node(end).is_none() {
                return Err(s.invalid(format!(
                    "relationship {id} points at node {end}, which the file does not contain"
                )));
            }
        }
        let props = read_props(s, names)?;
        graph.restore_relationship(id, kind, from, to, props);
    }
    Ok(next_rel)
}

/// Decode a whole `.graph` file into a new graph. The graph is returned only
/// when every section, every checksum and the content digest check out and
/// the input ends right after the last section.
pub(crate) fn read<R: Read>(input: R) -> Result<(Graph, ImportSummary), Error> {
    let mut input = In {
        inner: BufReader::with_capacity(BUFFER, input),
        offset: 0,
        content: None,
    };
    let mut magic = [0u8; 8];
    let got = input.fill(&mut magic)?;
    if magic[..got] != MAGIC[..got] {
        return Err(Error::BadMagic);
    }
    if got < magic.len() {
        return Err(Error::Truncated {
            offset: input.offset,
            context: "inside the magic bytes".into(),
        });
    }
    let mut version = [0u8; 4];
    input.exact(&mut version, || "inside the format version".into())?;
    let version = u32::from_le_bytes(version);
    if version > FORMAT_VERSION {
        return Err(Error::NewerVersion {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    if version == 0 {
        return Err(Error::Invalid {
            section: "header",
            offset: 8,
            reason: "format version 0 does not exist".into(),
        });
    }

    let manifest = read_section(&mut input, Section::Manifest, read_manifest)?;
    input.content = Some(Sha256::new());
    let names = read_section(&mut input, Section::Names, read_names)?;
    let schema = read_section(&mut input, Section::Schema, read_schema)?;
    let mut names = NameTable {
        used: vec![false; names.len()],
        names,
    };
    let mut graph = Graph::new();
    let next_node = read_section(&mut input, Section::Nodes, |s| {
        read_nodes(s, &mut graph, &mut names, &manifest)
    })?;
    let next_rel = read_section(&mut input, Section::Relationships, |s| {
        read_relationships(s, &mut graph, &mut names, &manifest)
    })?;
    let digest: [u8; 32] = input
        .content
        .take()
        .map(|content| content.finalize().into())
        .unwrap_or_default();
    let recorded = read_section(&mut input, Section::Done, |s| {
        let mut recorded = [0u8; 32];
        s.take(&mut recorded)?;
        Ok(recorded)
    })?;
    let invalid = |section: Section, offset: u64, reason: String| Error::Invalid {
        section: section.name(),
        offset,
        reason,
    };
    if recorded != digest {
        return Err(invalid(
            Section::Done,
            input.offset,
            format!(
                "the content digest is {}, the file says {}",
                hex(&digest),
                hex(&recorded)
            ),
        ));
    }
    let mut extra = [0u8; 1];
    if input.fill(&mut extra)? != 0 {
        return Err(invalid(
            Section::Done,
            input.offset - 1,
            "the file continues after the done section".into(),
        ));
    }
    if let Some(unused) = names.used.iter().position(|used| !used) {
        return Err(invalid(
            Section::Names,
            input.offset,
            format!("the name {:?} is never used", names.names[unused]),
        ));
    }

    graph.reset_next_ids((next_node, next_rel));
    graph.sync_indexes(&schema.indexes);
    graph.set_carried(Carried {
        schema: schema.source.clone(),
        uniques: schema.uniques.clone(),
        indexes: schema.indexes.clone(),
        meta: manifest.meta.clone(),
    });
    let indexes = schema
        .indexes
        .iter()
        .map(|spec| IndexDeclaration {
            kind: spec.kind.as_str(),
            type_name: spec.type_name.clone(),
            field: spec.field.clone(),
        })
        .collect();
    Ok((
        graph,
        ImportSummary {
            format_version: version,
            created_by: manifest.created_by,
            nodes: manifest.nodes,
            relationships: manifest.relationships,
            schema: schema.source,
            uniques: schema.uniques,
            indexes,
            meta: manifest.meta,
            content_sha256: hex(&digest),
        },
    ))
}

// ---------------------------------------------------------------------------
// Codes shared by the writer and the reader

mod tag {
    pub const NULL: u8 = 0x00;
    pub const FALSE: u8 = 0x01;
    pub const TRUE: u8 = 0x02;
    pub const INT: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const STRING: u8 = 0x05;
    pub const LIST: u8 = 0x06;
    pub const MAP: u8 = 0x07;
    pub const POINT: u8 = 0x08;
    pub const VECTOR: u8 = 0x09;
}

fn metric_code(metric: Metric) -> u8 {
    match metric {
        Metric::Cosine => 0,
        Metric::Dot => 1,
        Metric::L2 => 2,
    }
}

fn index_code(kind: IndexKind) -> u8 {
    match kind {
        IndexKind::Range => 0,
        IndexKind::Text => 1,
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
