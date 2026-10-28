//! The reader and writer services of `KLF-KSL.md` section 6.7: what an out
//! written in KLF Kira sees of one unit after the passes for its target.
//!
//! Every record is built when the view is made, so each service call is a
//! lookup. A unit is small and an out reads most of it, and building once
//! keeps block numbers and interned types stable between calls.
#![allow(clippy::cast_possible_wrap)]

mod body;
mod record;
mod tables;
mod values;

pub use record::{FIELDS, TABLES};

use klf_shader::{FunctionRef, Handle, Module, Span, Target};
use klf_shader_bind::Resource;
use klf_shader_valid::{ModuleInfo, ValidationError};
use record::Record;
use std::collections::HashMap;

/// One unit as an out reads it.
pub struct UnitView {
    module: Module,
    target: Target,
    tables: HashMap<(&'static str, i64), Vec<Record>>,
    /// The span of each statement, by block and position.
    statement_spans: Vec<Vec<Span>>,
    texts: Vec<String>,
    text_ids: HashMap<String, i64>,
    output: String,
    refusals: Vec<ValidationError>,
    misuse: Option<String>,
}

/// What an out produced for one unit.
#[derive(Debug)]
pub enum Written {
    Output(String),
    /// The out refused constructs; nothing it wrote is used.
    Refused(Vec<ValidationError>),
    /// The out misused a service or returned without a whole output. The
    /// message names the first misuse.
    Misused(String),
}

impl UnitView {
    /// The view of `module`, validated as `info`, with `resources` indexed
    /// by its global handles.
    #[must_use]
    pub fn new(
        mut module: Module,
        info: &ModuleInfo,
        resources: &[Option<Resource>],
        target: Target,
        debug: bool,
    ) -> Self {
        let types = body::intern_types(&mut module, info);
        let mut tables = HashMap::new();
        tables::build(&module, info, resources, target, debug, &mut tables);
        let statement_spans = body::build(&module, info, &types, &mut tables);
        Self {
            module,
            target,
            tables,
            statement_spans,
            texts: Vec::new(),
            text_ids: HashMap::new(),
            output: String::new(),
            refusals: Vec::new(),
            misuse: None,
        }
    }

    #[must_use]
    pub const fn target(&self) -> Target {
        self.target
    }

    fn misused(&mut self, message: String) -> i64 {
        self.misuse.get_or_insert(message);
        -1
    }

    fn records(&mut self, service: &str, table: &str, owner: i64) -> Option<&[Record]> {
        let Some(table) = TABLES.iter().copied().find(|known| *known == table) else {
            self.misused(format!("`{service}`: no table `{table}`"));
            return None;
        };
        if !self.tables.contains_key(&(table, owner)) {
            self.misused(format!("`{service}`: `{table}` has no owner {owner}"));
            return None;
        }
        self.tables.get(&(table, owner)).map(Vec::as_slice)
    }

    fn record(&mut self, service: &str, table: &str, owner: i64, index: i64) -> Option<&Record> {
        let count = self.records(service, table, owner)?.len();
        let Some(at) = usize::try_from(index).ok().filter(|at| *at < count) else {
            self.misused(format!(
                "`{service}`: `{table}` of owner {owner} has no record {index}"
            ));
            return None;
        };
        self.records(service, table, owner)
            .map(|records| &records[at])
    }

    fn known_field(&mut self, service: &str, table: &str, field: &str) -> bool {
        let known = FIELDS
            .iter()
            .any(|(name, fields)| *name == table && fields.contains(&field));
        if !known {
            self.misused(format!("`{service}`: `{table}` has no field `{field}`"));
        }
        known
    }

    /// `klf.shader.read.count`: how many records `table` holds under
    /// `owner`.
    pub fn count(&mut self, table: &str, owner: i64) -> i64 {
        self.records("klf.shader.read.count", table, owner)
            .map_or(-1, |records| records.len() as i64)
    }

    /// `klf.shader.read.kind`: the record's kind, as a text id.
    pub fn kind(&mut self, table: &str, owner: i64, index: i64) -> i64 {
        let kind = self
            .record("klf.shader.read.kind", table, owner, index)
            .map(|record| record.kind.clone());
        kind.map_or(-1, |kind| self.intern(kind))
    }

    /// `klf.shader.read.int`: an integer field. Item -1 answers how many
    /// items the field holds.
    pub fn int(&mut self, table: &str, owner: i64, index: i64, field: &str, item: i64) -> i64 {
        const SERVICE: &str = "klf.shader.read.int";
        if !self.known_field(SERVICE, table, field) {
            return -1;
        }
        let values = self
            .record(SERVICE, table, owner, index)
            .map(|record| record.ints_of(field).to_vec());
        match values {
            Some(values) => self.item(SERVICE, field, &values, item, -1),
            None => -1,
        }
    }

    /// `klf.shader.read.text`: a text field, as a text id. Item -1 answers
    /// how many items the field holds, as a number.
    pub fn text(&mut self, table: &str, owner: i64, index: i64, field: &str, item: i64) -> i64 {
        const SERVICE: &str = "klf.shader.read.text";
        if !self.known_field(SERVICE, table, field) {
            return -1;
        }
        let values = self
            .record(SERVICE, table, owner, index)
            .map(|record| record.texts_of(field).to_vec());
        let Some(values) = values else {
            return -1;
        };
        if item == -1 {
            return values.len() as i64;
        }
        let ids: Vec<i64> = values.into_iter().map(|text| self.intern(text)).collect();
        self.item(SERVICE, field, &ids, item, -1)
    }

    fn item(&mut self, service: &str, field: &str, values: &[i64], item: i64, absent: i64) -> i64 {
        if item == -1 {
            return values.len() as i64;
        }
        if values.is_empty() && item == 0 {
            return absent;
        }
        match usize::try_from(item).ok().and_then(|at| values.get(at)) {
            Some(value) => *value,
            None => self.misused(format!("`{service}`: `{field}` has no item {item}")),
        }
    }

    fn intern(&mut self, text: String) -> i64 {
        if let Some(id) = self.text_ids.get(&text) {
            return *id;
        }
        let id = self.texts.len() as i64;
        self.texts.push(text.clone());
        self.text_ids.insert(text, id);
        id
    }

    /// The text of a text id; -1 is the empty text.
    pub fn text_of(&mut self, id: i64) -> Option<&str> {
        if id == -1 {
            return Some("");
        }
        if let Some(at) = usize::try_from(id).ok().filter(|at| *at < self.texts.len()) {
            Some(&self.texts[at])
        } else {
            self.misused(format!("`klf.shader.read.text_len`: no text {id}"));
            None
        }
    }

    /// `klf.shader.out.write`.
    pub fn write(&mut self, text: &str) {
        self.output.push_str(text);
    }

    /// `klf.shader.out.refuse`: a construct the target cannot spell, at
    /// `expression` of function number `function` (-1 for the function or
    /// the unit).
    pub fn refuse(&mut self, message: &str, function: i64, expression: i64) {
        let span = self.span(function, expression);
        self.refuse_with(message, span);
    }

    /// `klf.shader.out.refuse_at`: a construct the target cannot spell, at
    /// the record `index` of `table` under `owner`. A table whose records
    /// have no span of their own, or a record that does not exist, gives
    /// none.
    pub fn refuse_at(&mut self, message: &str, table: &str, owner: i64, index: i64) {
        let span = self.record_span(table, owner, index).unwrap_or_default();
        self.refuse_with(message, span);
    }

    fn refuse_with(&mut self, message: &str, span: Span) {
        self.refusals.push(ValidationError::keyed(
            "B3",
            "shader.target-unsupported",
            format!("{}: {message}", self.target.name()),
            span,
        ));
    }

    fn record_span(&self, table: &str, owner: i64, index: i64) -> Option<Span> {
        let at = usize::try_from(index).ok()?;
        let raw = u32::try_from(index).ok()?;
        let module = &self.module;
        let within = |count: usize| (at < count).then_some(());
        Some(match table {
            "types" => {
                within(module.types.len())?;
                module.types.span(Handle::new(raw))
            }
            "constants" => {
                within(module.constants.len())?;
                module.constants.span(Handle::new(raw))
            }
            "overrides" => {
                within(module.overrides.len())?;
                module.overrides.span(Handle::new(raw))
            }
            "globals" => {
                within(module.globals.len())?;
                module.globals.span(Handle::new(raw))
            }
            "functions" => self.span(index, -1),
            "statements" => *self
                .statement_spans
                .get(usize::try_from(owner).ok()?)?
                .get(at)?,
            "locals" => {
                let function = module.function(self.function_ref(owner)?);
                within(function.locals.len())?;
                function.locals.span(Handle::new(raw))
            }
            "expressions" if owner == -1 => {
                within(module.global_expressions.len())?;
                module.global_expressions.span(Handle::new(raw))
            }
            "expressions" => self.span(owner, index),
            _ => return None,
        })
    }

    fn function_ref(&self, function: i64) -> Option<FunctionRef> {
        let at = usize::try_from(function).ok()?;
        let functions = self.module.functions.len();
        if at < functions {
            Some(FunctionRef::Function(Handle::new(u32::try_from(at).ok()?)))
        } else {
            (at - functions < self.module.entry_points.len())
                .then_some(FunctionRef::Entry(at - functions))
        }
    }

    fn span(&self, function: i64, expression: i64) -> Span {
        let Some(which) = self.function_ref(function) else {
            return Span::default();
        };
        let expressions = &self.module.function(which).expressions;
        match u32::try_from(expression).ok().map(Handle::new) {
            Some(handle) if handle.index() < expressions.len() => expressions.span(handle),
            _ => match which {
                FunctionRef::Function(handle) => self.module.functions.span(handle),
                FunctionRef::Entry(index) => self.module.entry_points[index].span,
            },
        }
    }

    /// The module the view reads, with every expression type interned.
    #[must_use]
    pub const fn module(&self) -> &Module {
        &self.module
    }

    /// Ends the call: `returned` is what `emit` answered.
    #[must_use]
    pub fn finish(self, returned: bool) -> Written {
        if let Some(misuse) = self.misuse {
            return Written::Misused(misuse);
        }
        match (returned, self.refusals.is_empty()) {
            (true, true) => Written::Output(self.output),
            (_, false) => Written::Refused(self.refusals),
            (false, true) => Written::Misused("`emit` failed without refusing anything".to_owned()),
        }
    }
}
