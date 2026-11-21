//! Portable PDB documents and IL sequence points embedded in the managed PE.

use std::{io::Write, path::Path};

use clrmeta::{
    heaps::{BlobHeap, GuidHeap},
    writer::Writer,
};
use flate2::{Compression, write::DeflateEncoder};
use sha2::{Digest, Sha256};

use super::{image_offset, metadata_root};
use crate::{Diagnostic, Span};

const DOCUMENT: u64 = 1 << 0x30;
const METHOD_DEBUG_INFO: u64 = 1 << 0x31;
const METHOD_DEF: u64 = 1 << 0x06;
const STANDALONE_SIG: u64 = 1 << 0x11;
const DEBUG_ENTRY_SIZE: u32 = 28;
const SHA256_ID: [u8; 16] = [
    0x0f, 0xd0, 0x29, 0x88, 0xb8, 0x11, 0x13, 0x42, 0x87, 0x8b, 0x77, 0x0e, 0x85, 0x97, 0xac, 0x16,
];

struct Document {
    base: usize,
    lines: Vec<usize>,
    name: u32,
    hash: u32,
}

pub(super) struct Symbols<'src> {
    sources: &'src [&'src str],
    documents: Vec<Document>,
    methods: Vec<(u32, u32)>,
    blobs: BlobHeap,
    guids: GuidHeap,
}

impl<'src> Symbols<'src> {
    pub(super) fn new(sources: &'src [&'src str], names: &[&Path]) -> Self {
        let mut blobs = BlobHeap::new();
        let mut guids = GuidHeap::new();
        guids.add(&SHA256_ID);
        let mut base = 0;
        let documents = sources
            .iter()
            .zip(names)
            .map(|(&source, name)| {
                let path = blobs.add(name.to_string_lossy().as_bytes());
                let mut name_blob = Writer::new();
                name_blob.write_u8(0); // one path part, no separator
                name_blob.write_compressed_uint(path);
                let document = Document {
                    base,
                    lines: std::iter::once(0)
                        .chain(source.match_indices('\n').map(|(byte, _)| byte + 1))
                        .collect(),
                    name: blobs.add(name_blob.as_slice()),
                    hash: blobs.add(&Sha256::digest(source.as_bytes())),
                };
                base += source.len() + 1;
                document
            })
            .collect();
        Self {
            sources,
            documents,
            methods: Vec::new(),
            blobs,
            guids,
        }
    }

    pub(super) fn record(
        &mut self,
        method: usize,
        span: Span,
        locals: u32,
        points: &[(usize, Span)],
        code_size: usize,
    ) {
        if span.end <= span.start {
            return;
        }
        let Some(id) = self
            .documents
            .partition_point(|document| document.base <= span.start)
            .checked_sub(1)
        else {
            return;
        };
        let document = &self.documents[id];
        let source = self.sources[id];
        let mut data = Writer::new();
        data.write_compressed_uint(locals & 0x00ff_ffff);
        let mut previous = None;
        for &(offset, point) in points {
            if offset >= code_size
                || offset >= 0x2000_0000
                || point.start < span.start
                || point.start >= span.end
                || point.end <= point.start
            {
                continue;
            }
            let local = point.start - document.base;
            let line = document.lines.partition_point(|&start| start <= local);
            let Some(text) = source.get(document.lines[line - 1]..local) else {
                continue;
            };
            let column = text.encode_utf16().count() + 1;
            if column >= u16::MAX as usize || line >= 0x1000_0000 || line == 0xfeefee {
                continue;
            }
            if previous.is_some_and(|(last_offset, last_line, _)| {
                last_offset == offset || last_line == line
            }) {
                continue;
            }
            data.write_compressed_uint(
                (offset - previous.map_or(0, |(offset, _, _)| offset)) as u32,
            );
            data.write_compressed_uint(0); // one source line
            data.write_compressed_uint(1); // one column
            if let Some((_, last_line, last_column)) = previous {
                compressed_signed(&mut data, line as i32 - last_line as i32);
                compressed_signed(&mut data, column as i32 - last_column as i32);
            } else {
                data.write_compressed_uint(line as u32);
                data.write_compressed_uint(column as u32);
            }
            previous = Some((offset, line, column));
        }
        if previous.is_some() {
            self.methods
                .resize(self.methods.len().max(method + 1), (0, 0));
            self.methods[method] = (id as u32 + 1, self.blobs.add(data.as_slice()));
        }
    }

    pub(super) fn finish(
        mut self,
        name: &str,
        entry: u32,
        methods: usize,
        locals: usize,
    ) -> Result<Directory, Diagnostic> {
        self.methods.resize(methods, (0, 0));
        let mut header = Writer::new();
        header.write_bytes(&[0; 20]); // deterministic ID filled after hashing
        header.write_u32(entry);
        header.write_u64(METHOD_DEF | if locals > 0 { STANDALONE_SIG } else { 0 });
        header.write_u32(methods as u32);
        if locals > 0 {
            header.write_u32(locals as u32);
        }

        let mut tables = Writer::new();
        tables.write_u32(0);
        tables.write_u8(2); // version 2.0
        tables.write_u8(0);
        let wide_blobs = self.blobs.data().len() > u16::MAX as usize;
        tables.write_u8(if wide_blobs { 4 } else { 0 });
        tables.write_u8(1);
        tables.write_u64(DOCUMENT | METHOD_DEBUG_INFO);
        tables.write_u64(0); // neither table is sorted
        tables.write_u32(self.documents.len() as u32);
        tables.write_u32(methods as u32);
        for document in &self.documents {
            index(&mut tables, document.name, wide_blobs);
            tables.write_u16(1); // SHA-256
            index(&mut tables, document.hash, wide_blobs);
            tables.write_u16(0); // language is unspecified
        }
        for (document, points) in self.methods {
            index(&mut tables, document, self.documents.len() >= 65_536);
            index(&mut tables, points, wide_blobs);
        }

        let mut pdb = metadata_root(
            "PDB v1.0",
            [
                ("#Pdb", header.as_slice()),
                ("#~", tables.as_slice()),
                ("#Strings", &[0]),
                ("#US", &[0]),
                ("#GUID", self.guids.data()),
                ("#Blob", self.blobs.data()),
            ],
        )?;
        let id = *blake3::hash(&pdb)
            .as_bytes()
            .first_chunk::<20>()
            .expect("hash length");
        // The first stream begins with the 20-byte PDB ID.
        let root_size = pdb.len()
            - [
                header.len(),
                tables.len(),
                1,
                1,
                self.guids.data().len(),
                self.blobs.data().len(),
            ]
            .into_iter()
            .map(|size| size.next_multiple_of(4))
            .sum::<usize>();
        pdb[root_size..root_size + id.len()].copy_from_slice(&id);
        let mut codeview = Writer::new();
        codeview.write_bytes(b"RSDS");
        codeview.write_bytes(&id[..16]);
        codeview.write_u32(1);
        codeview.write_bytes(name.as_bytes());
        codeview.write_bytes(b".pdb\0");

        let mut deflate = DeflateEncoder::new(Vec::new(), Compression::fast());
        deflate.write_all(&pdb).map_err(compression_error)?;
        let compressed = deflate.finish().map_err(compression_error)?;
        let mut embedded = Writer::new();
        embedded.write_bytes(b"MPDB");
        embedded.write_u32(image_offset(pdb.len())?);
        embedded.write_bytes(&compressed);
        Ok(Directory {
            id,
            codeview: codeview.into_inner(),
            embedded: embedded.into_inner(),
        })
    }
}

fn index(writer: &mut Writer, value: u32, wide: bool) {
    if wide {
        writer.write_u32(value);
    } else {
        writer.write_u16(value as u16);
    }
}

fn compressed_signed(writer: &mut Writer, value: i32) {
    let sign = u32::from(value < 0);
    let payload = (value as u32) << 1 | sign;
    match value {
        -64..=63 => writer.write_u8((payload & 0x7f) as u8),
        -8192..=8191 => writer.write_bytes(&((payload & 0x3fff) as u16 | 0x8000).to_be_bytes()),
        _ => writer.write_bytes(&((payload & 0x1fff_ffff) | 0xc000_0000).to_be_bytes()),
    }
}

fn compression_error(error: std::io::Error) -> Diagnostic {
    Diagnostic::new(
        "E0403",
        Span::default(),
        format!("could not compress debug symbols: {error}"),
    )
}

pub(super) struct Directory {
    pub(super) id: [u8; 20],
    codeview: Vec<u8>,
    embedded: Vec<u8>,
}

impl Directory {
    pub(super) const ENTRIES_SIZE: u32 = 2 * DEBUG_ENTRY_SIZE;

    pub(super) fn size(&self) -> usize {
        Self::ENTRIES_SIZE as usize + self.codeview.len() + self.embedded.len()
    }

    pub(super) fn append(&self, section: &mut Writer, base_rva: u32, file_offset: u32) {
        section.align(4);
        let mut offset = section.len() as u32 + Self::ENTRIES_SIZE;
        let stamp = u32::from_le_bytes(self.id[16..].try_into().expect("four-byte stamp"));
        for (kind, stamp, major, minor, data) in [
            (2, stamp, 0x0100, 0x504d, &self.codeview),
            (17, 0, 0x0100, 0x0100, &self.embedded),
        ] {
            section.write_u32(0);
            section.write_u32(stamp);
            section.write_u16(major);
            section.write_u16(minor);
            section.write_u32(kind);
            section.write_u32(data.len() as u32);
            section.write_u32(base_rva + offset);
            section.write_u32(file_offset + offset);
            offset += data.len() as u32;
        }
        section.write_bytes(&self.codeview);
        section.write_bytes(&self.embedded);
    }
}
