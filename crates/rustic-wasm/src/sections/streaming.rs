use super::*;
use crate::binary::{function_bodies, function_types, header, section_payload};

/// Incrementally decode a version 1 module. Complete sections are processed
/// in arrival order, including function bodies. The bounded byte snapshot
/// also backs borrowed names and data at completion.
pub struct StreamingDecoder<'token> {
    limits: BinaryLimits,
    decoder: Decoder<'token>,
    bytes: Vec<u8>,
    offset: usize,
    previous: SectionKind,
    sections: usize,
    deferred: Vec<DeferredSection>,
    declarations: Declarations<'static>,
    function_types: Vec<u32>,
    functions: Vec<Declaration<Function>>,
    data_segments: u32,
    failure: Option<BinaryError>,
    pending: Option<BinaryError>,
}

struct DeferredSection {
    kind: SectionKind,
    start: usize,
    end: usize,
}

impl<'token> StreamingDecoder<'token> {
    /// Start a streaming decoder with the same admission limits as a complete
    /// module. Cancellation is checked between and within sections.
    pub fn new(limits: DecodeLimits, cancellation: &'token CancellationToken) -> Self {
        Self {
            limits: limits.binary,
            decoder: Decoder::new(limits, cancellation),
            bytes: Vec::new(),
            offset: 0,
            previous: SectionKind::Custom,
            sections: 0,
            deferred: Vec::new(),
            declarations: Declarations::default(),
            function_types: Vec::new(),
            functions: Vec::new(),
            data_segments: 0,
            failure: None,
            pending: Some(BinaryError {
                offset: 0,
                kind: Fault::UnexpectedEnd,
            }),
        }
    }

    /// Append a chunk and decode each newly completed section. A fatal error
    /// is sticky and leaves subsequent chunks unread.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), BinaryError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.push_chunk(bytes);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    fn push_chunk(&mut self, bytes: &[u8]) -> Result<(), BinaryError> {
        let source = Reader::new(&[], self.offset);
        poll(&source, self.decoder.cancellation)?;
        let len = self.bytes.len();
        let end = len
            .checked_add(bytes.len())
            .filter(|&end| end <= self.limits.module_bytes)
            .ok_or_else(|| source.error(Fault::LimitExceeded))?;
        if end > self.bytes.capacity() {
            let capacity = end
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limits.module_bytes);
            self.bytes
                .try_reserve_exact(capacity - len)
                .map_err(|_| source.error(Fault::LimitExceeded))?;
        }
        self.bytes.extend_from_slice(bytes);

        if self.offset == 0 {
            let mut source = Reader::new(&self.bytes, 0);
            match header(&mut source) {
                Err(error) if error.kind == Fault::UnexpectedEnd => {
                    self.pending = Some(error);
                    return Ok(());
                }
                Err(error) => return Err(error),
                Ok(()) => self.offset = source.position(),
            }
        }

        self.pending = None;
        while self.offset < self.bytes.len() {
            let mut source = Reader::new(&self.bytes[self.offset..], self.offset);
            poll(&source, self.decoder.cancellation)?;
            if self.sections >= self.limits.sections {
                return Err(source.error(Fault::LimitExceeded));
            }
            let (kind, mut payload) = match section_payload(&mut source, self.previous) {
                Err(error) if error.kind == Fault::UnexpectedEnd => {
                    self.pending = Some(error);
                    return Ok(());
                }
                other => other?,
            };
            let start = payload.position();
            let end = source.position();
            match kind {
                SectionKind::Function => {
                    self.function_types =
                        function_types(payload, self.limits.functions, self.decoder.cancellation)?;
                }
                SectionKind::Code => {
                    let bodies = function_bodies(
                        payload,
                        &self.function_types,
                        self.limits,
                        self.decoder.cancellation,
                    )?;
                    self.functions = self.decoder.functions(bodies.into_iter(), &source)?;
                }
                SectionKind::DataCount => {
                    self.declarations.data_count = Some(payload.u32()?);
                    payload.finish()?;
                }
                SectionKind::Data
                | SectionKind::Custom
                | SectionKind::Import
                | SectionKind::Export => {
                    if kind == SectionKind::Data {
                        self.data_segments = payload.u32()?;
                    } else if kind == SectionKind::Custom {
                        payload.name()?;
                    }
                    self.deferred
                        .try_reserve(1)
                        .map_err(|_| source.error(Fault::LimitExceeded))?;
                    self.deferred.push(DeferredSection { kind, start, end });
                }
                kind => self
                    .decoder
                    .owned_section(kind, &mut payload, &mut self.declarations)?,
            }
            self.sections += 1;
            self.offset = end;
            if kind != SectionKind::Custom {
                self.previous = kind;
            }
        }
        Ok(())
    }

    /// Finish at EOF, giving the consumer the original bytes and decoded
    /// module. The returned byte allocation can be retained for reflection.
    pub fn finish<T>(
        mut self,
        consume: impl FnOnce(&[u8], DecodedModule<'_>) -> T,
    ) -> Result<(Vec<u8>, T), BinaryError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let source = Reader::new(&[], self.offset);
        poll(&source, self.decoder.cancellation)?;
        if let Some(error) = self.pending {
            return Err(error);
        }
        if self.functions.len() != self.function_types.len() {
            return Err(source.error(Fault::FunctionCountMismatch));
        }
        if self
            .declarations
            .data_count
            .is_some_and(|count| count != self.data_segments)
        {
            return Err(source.error(Fault::DataCountMismatch));
        }
        let mut declarations = self.declarations;
        let mut custom_sections = Vec::new();
        for section in self.deferred {
            let mut reader = Reader::new(&self.bytes[section.start..section.end], section.start);
            poll(&reader, self.decoder.cancellation)?;
            if section.kind == SectionKind::Custom {
                let name = reader.name()?;
                custom_sections
                    .try_reserve(1)
                    .map_err(|_| reader.error(Fault::LimitExceeded))?;
                custom_sections.push(crate::Section {
                    kind: section.kind,
                    name: Some(name),
                    offset: reader.position(),
                    bytes: reader.remaining(),
                });
            } else {
                self.decoder
                    .borrowing_section(section.kind, &mut reader, &mut declarations)?;
            }
        }
        poll(&source, self.decoder.cancellation)?;
        let result = consume(
            &self.bytes,
            DecodedModule {
                declarations,
                functions: self.functions,
                custom_sections,
            },
        );
        Ok((self.bytes, result))
    }
}
