//! Typed, lenient views of the JSON files harness2 writes, for the views that read many of
//! them at once (task lists, sweep lists, spend, judged rows, sweep records, audit metadata).
//!
//! Each view names only the fields the reader uses. Every other field, however large (stage
//! notes, tracebacks, judge details), is skipped while parsing and never allocated. What is
//! kept is bounded as well:
//!
//! - a string longer than [`MAX_FIELD_BYTES`] is not kept ([`Loose::Long`]); messages
//!   ([`Note`]) are cut at [`MAX_NOTE_BYTES`] instead;
//! - lists that only feed a number are reduced while they are read (stage spend, lengths);
//! - row lists stop growing at a cap ([`rows`]) and say so.
//!
//! So memory follows the number of rows kept, never the size of the files. Leaf values apply
//! Python's semantics to whatever type harness2 wrote; a value of an unexpected type never
//! fails the parse (containers degrade to their defaults). A key repeated inside one object
//! does fail it (harness2's `json.dump` never writes one), except mode keys, where the last
//! one wins as in `json.loads`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::marker::PhantomData;

use h2s_api::Mode;
use serde::de::{self, Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::pyfmt::{self, PySum};

/// Longest string kept in an identifier field (task ids, labels, statuses, model names).
/// Real ones are a few dozen bytes; a longer value is treated as absent.
pub(crate) const MAX_FIELD_BYTES: usize = 1024;

/// Longest message kept from an error field ([`Note`]); longer ones are cut here.
pub(crate) const MAX_NOTE_BYTES: usize = 4096;

/// The longest prefix of `s` of at most `max` bytes that ends on a character boundary.
fn prefix(s: &str, max: usize) -> &str {
    let mut i = max.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    s.get(..i).unwrap_or_default()
}

/// Any JSON value, reduced while parsing to what the readers need: scalars are kept,
/// containers only report how many items they hold (their contents are skipped).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum Loose {
    #[default]
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    /// A string longer than [`MAX_FIELD_BYTES`], not kept (its length in bytes).
    Long(usize),
    Seq(usize),
    Map(usize),
}

impl Loose {
    /// Python truthiness.
    pub(crate) fn truthy(&self) -> bool {
        match self {
            Loose::Null => false,
            Loose::Bool(b) => *b,
            Loose::Num(n) => n.as_f64().is_some_and(|f| f != 0.0),
            Loose::Str(s) => !s.is_empty(),
            Loose::Long(_) => true,
            Loose::Seq(n) | Loose::Map(n) => *n > 0,
        }
    }

    /// `float(x or 0.0)`.
    pub(crate) fn number(&self) -> f64 {
        match self {
            Loose::Num(n) => n.as_f64().unwrap_or(0.0),
            Loose::Bool(true) => 1.0,
            Loose::Str(s) if !s.is_empty() => pyfmt::strip(s)
                .replace('_', "")
                .parse::<f64>()
                .unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// A text field: strings verbatim, numbers and booleans as JSON, anything else empty.
    pub(crate) fn text(&self) -> String {
        match self {
            Loose::Str(s) => s.clone(),
            Loose::Bool(b) => b.to_string(),
            Loose::Num(n) => n.to_string(),
            Loose::Null | Loose::Long(_) | Loose::Seq(_) | Loose::Map(_) => String::new(),
        }
    }

    /// [`Loose::text`], moving the string out.
    pub(crate) fn into_text(self) -> String {
        match self {
            Loose::Str(s) => s,
            other => other.text(),
        }
    }

    /// A non-negative whole number, else 0.
    pub(crate) fn count(&self) -> u32 {
        match self {
            Loose::Num(n) => n
                .as_u64()
                .map(|x| u32::try_from(x).unwrap_or(u32::MAX))
                .or_else(|| n.as_f64().and_then(pyfmt::exact_u32))
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// The length of a list (0 for anything else).
    pub(crate) fn len(&self) -> u32 {
        match self {
            Loose::Seq(n) => u32::try_from(*n).unwrap_or(u32::MAX),
            _ => 0,
        }
    }

    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Loose::Num(n) => n.as_f64(),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Loose::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A name to match on (domain and task names in a sweep record): strings and other
    /// scalars as [`Loose::text`] spells them, `None` for containers and over-long strings.
    fn name(self) -> Option<String> {
        match self {
            Loose::Long(_) | Loose::Seq(_) | Loose::Map(_) => None,
            other => Some(other.into_text()),
        }
    }
}

/// Reads one [`Loose`]: strings up to `cap` bytes are kept, longer ones are cut there
/// (`cut`) or reduced to [`Loose::Long`].
struct LooseVisitor {
    cap: usize,
    cut: bool,
}

impl LooseVisitor {
    fn string(&self, v: &str) -> Loose {
        if v.len() <= self.cap {
            Loose::Str(v.to_owned())
        } else if self.cut {
            Loose::Str(prefix(v, self.cap).to_owned())
        } else {
            Loose::Long(v.len())
        }
    }
}

impl<'de> Visitor<'de> for LooseVisitor {
    type Value = Loose;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Loose, E> {
        Ok(Loose::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Loose, E> {
        Ok(Loose::Num(v.into()))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Loose, E> {
        Ok(Loose::Num(v.into()))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Loose, E> {
        Ok(serde_json::Number::from_f64(v).map_or(Loose::Null, Loose::Num))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Loose, E> {
        Ok(self.string(v))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Loose, E> {
        if v.len() <= self.cap {
            Ok(Loose::Str(v))
        } else {
            Ok(self.string(&v))
        }
    }

    fn visit_unit<E: de::Error>(self) -> Result<Loose, E> {
        Ok(Loose::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Loose, E> {
        Ok(Loose::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Loose, A::Error> {
        let mut n = 0_usize;
        while seq.next_element::<IgnoredAny>()?.is_some() {
            n += 1;
        }
        Ok(Loose::Seq(n))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Loose, A::Error> {
        let mut n = 0_usize;
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {
            n += 1;
        }
        Ok(Loose::Map(n))
    }
}

impl<'de> Deserialize<'de> for Loose {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(LooseVisitor {
            cap: MAX_FIELD_BYTES,
            cut: false,
        })
    }
}

/// A message field (an error): like [`Loose`], but a string past [`MAX_NOTE_BYTES`] keeps
/// its first [`MAX_NOTE_BYTES`] bytes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Note(pub(crate) Loose);

impl<'de> Deserialize<'de> for Note {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(LooseVisitor {
            cap: MAX_NOTE_BYTES,
            cut: true,
        })
        .map(Note)
    }
}

/// Visitor methods for every JSON type that is not the one wanted: skip the value and
/// return `$default`.
macro_rules! otherwise {
    ($t:ty, $default:expr) => {
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<$t, A::Error> {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok($default)
        }

        fn visit_bool<E: de::Error>(self, _: bool) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_i64<E: de::Error>(self, _: i64) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_u64<E: de::Error>(self, _: u64) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_f64<E: de::Error>(self, _: f64) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_str<E: de::Error>(self, _: &str) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_unit<E: de::Error>(self) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_none<E: de::Error>(self) -> Result<$t, E> {
            Ok($default)
        }
    };
}

/// Like [`otherwise`] for visitors that want a list: also skips objects.
macro_rules! otherwise_not_seq {
    ($t:ty, $default:expr) => {
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<$t, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok($default)
        }

        fn visit_bool<E: de::Error>(self, _: bool) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_i64<E: de::Error>(self, _: i64) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_u64<E: de::Error>(self, _: u64) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_f64<E: de::Error>(self, _: f64) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_str<E: de::Error>(self, _: &str) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_unit<E: de::Error>(self) -> Result<$t, E> {
            Ok($default)
        }

        fn visit_none<E: de::Error>(self) -> Result<$t, E> {
            Ok($default)
        }
    };
}

/// An object read into `T`; any other JSON value becomes `T::default()` (Python's `x.get`
/// on a non-dict would fail upstream; the reader degrades instead).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Obj<T>(pub(crate) T);

struct ObjVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de> + Default> Visitor<'de> for ObjVisitor<T> {
    type Value = Obj<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Obj<T>, A::Error> {
        T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Obj)
    }

    otherwise!(Obj<T>, Obj(T::default()));
}

impl<'de, T: Deserialize<'de> + Default> Deserialize<'de> for Obj<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ObjVisitor(PhantomData))
    }
}

/// `{"agg": T, "seq": T}` keyed by mode token (CLI names accepted too); other keys are
/// skipped, a non-object is empty. As in upstream's reader the on-disk token wins over a CLI
/// name for the same mode, and a repeated key keeps its last value (`json.loads`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModeMap<T>(Vec<(Mode, bool, T)>);

impl<T> Default for ModeMap<T> {
    fn default() -> Self {
        ModeMap(Vec::new())
    }
}

impl<T> ModeMap<T> {
    pub(crate) fn get(&self, mode: Mode) -> Option<&T> {
        self.0
            .iter()
            .find(|(m, _, _)| *m == mode)
            .map(|(_, _, v)| v)
    }

    pub(crate) fn modes(&self) -> impl Iterator<Item = Mode> + '_ {
        self.0.iter().map(|(m, _, _)| *m)
    }
}

struct ModeMapVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for ModeMapVisitor<T> {
    type Value = ModeMap<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object keyed by mode")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ModeMap<T>, A::Error> {
        let mut out: Vec<(Mode, bool, T)> = Vec::new();
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            let Some(mode) = Mode::from_token(&key) else {
                map.next_value::<IgnoredAny>()?;
                continue;
            };
            let token = key == mode.token();
            let value: T = map.next_value()?;
            match out.iter_mut().find(|(m, _, _)| *m == mode) {
                // A CLI name never replaces the on-disk token.
                Some(slot) if slot.1 && !token => {}
                Some(slot) => *slot = (mode, token, value),
                None => out.push((mode, token, value)),
            }
        }
        Ok(ModeMap(out))
    }

    otherwise!(ModeMap<T>, ModeMap::default());
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ModeMap<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ModeMapVisitor(PhantomData))
    }
}

/// For `#[serde(deserialize_with = "present")]` on an `Option` field of a
/// `#[serde(default)]` struct: `Some` whenever the key exists, even when its value is `null`
/// (Python's `"key" in record`).
pub(crate) fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// Parse a whole document that must be a JSON object into `T`.
pub(crate) fn object<T: for<'de> Deserialize<'de> + Default>(
    bytes: &[u8],
) -> Result<T, serde_json::Error> {
    struct Top<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Top<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a JSON object")
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
            T::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let v = (&mut d).deserialize_any(Top(PhantomData))?;
    d.end()?;
    Ok(v)
}

/// Streams a JSON array: each element is read as `L`, converted, and kept when `convert`
/// returns a row, up to `cap` rows.
struct RowsVisitor<L, F> {
    cap: usize,
    convert: F,
    _l: PhantomData<fn() -> L>,
}

impl<'de, L, T, F> Visitor<'de> for RowsVisitor<L, F>
where
    L: Deserialize<'de>,
    F: FnMut(L) -> Option<T>,
{
    type Value = (Vec<T>, bool);

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON array")
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while out.len() < self.cap {
            let Some(item) = seq.next_element::<L>()? else {
                return Ok((out, false));
            };
            if let Some(row) = (self.convert)(item) {
                out.push(row);
            }
        }
        // At the cap: one more row is an overflow; the rest is only checked for syntax.
        let mut overflow = false;
        while !overflow {
            let Some(item) = seq.next_element::<L>()? else {
                break;
            };
            overflow = (self.convert)(item).is_some();
        }
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok((out, overflow))
    }

    otherwise_not_seq!((Vec<T>, bool), (Vec::new(), false));
}

/// Parse a document that should be a JSON array, converting each element as it is read and
/// keeping at most `cap` converted rows: `(rows, more rows than cap)`. A document that is
/// not an array has no rows.
pub(crate) fn rows<L, T>(
    bytes: &[u8],
    cap: usize,
    convert: impl FnMut(L) -> Option<T>,
) -> Result<(Vec<T>, bool), serde_json::Error>
where
    L: for<'de> Deserialize<'de>,
{
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let v = (&mut d).deserialize_any(RowsVisitor {
        cap,
        convert,
        _l: PhantomData,
    })?;
    d.end()?;
    Ok(v)
}

// ------------------------------------------------------------------------------------
// The views.
// ------------------------------------------------------------------------------------

/// What the multi-task views need from one task's `summary.json`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SummaryLite {
    pub(crate) task_id: Loose,
    pub(crate) domain: Loose,
    pub(crate) status: Loose,
    pub(crate) errors: Loose,
    #[serde(deserialize_with = "present")]
    pub(crate) modes: Option<ModeMap<Obj<ModeLite>>>,
    #[serde(deserialize_with = "present")]
    pub(crate) arms: Option<ModeMap<Obj<ModeLite>>>,
    pub(crate) selections: ModeMap<Obj<SelectionLite>>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ModeLite {
    pub(crate) ok: Loose,
    pub(crate) fell_back: Loose,
    pub(crate) stages: StageSpend,
}

/// A mode's `stages` list reduced, while it is read, to upstream's
/// `sum(float(s.get("usd") or 0.0) for s in stages)`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct StageSpend(pub(crate) PySum);

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct StageLite {
    usd: Loose,
}

struct StageSpendVisitor;

impl<'de> Visitor<'de> for StageSpendVisitor {
    type Value = StageSpend;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of stages")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StageSpend, A::Error> {
        let mut acc = PySum::default();
        while let Some(Obj(stage)) = seq.next_element::<Obj<StageLite>>()? {
            acc.add(stage.usd.number());
        }
        Ok(StageSpend(acc))
    }

    otherwise_not_seq!(StageSpend, StageSpend::default());
}

impl<'de> Deserialize<'de> for StageSpend {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StageSpendVisitor)
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SelectionLite {
    pub(crate) ok: Loose,
    pub(crate) chosen_label: Loose,
    pub(crate) chosen_is_base: Loose,
    #[serde(deserialize_with = "present")]
    pub(crate) chosen_tree_is_base: Option<Loose>,
    pub(crate) usd: Loose,
}

impl SummaryLite {
    /// Upstream `_modes_of`: `modes` when the key exists, else the legacy `arms`.
    fn mode_records(&self) -> Option<&ModeMap<Obj<ModeLite>>> {
        self.modes.as_ref().or(self.arms.as_ref())
    }

    /// Modes with a record or a selection, in canonical order.
    pub(crate) fn modes_present(&self) -> Vec<Mode> {
        let mut modes: Vec<Mode> = self
            .mode_records()
            .into_iter()
            .flat_map(ModeMap::modes)
            .chain(self.selections.modes())
            .collect();
        modes.sort();
        modes.dedup();
        modes
    }

    pub(crate) fn mode(&self, mode: Mode) -> Option<&ModeLite> {
        self.mode_records()?.get(mode).map(|o| &o.0)
    }

    pub(crate) fn selection(&self, mode: Mode) -> Option<&SelectionLite> {
        self.selections.get(mode).map(|o| &o.0)
    }

    /// Upstream `_spend_and_fallbacks` for one summary and mode: `(fell_back, stage spend,
    /// selection spend)`.
    pub(crate) fn mode_spend(&self, mode: Mode) -> (u32, f64, f64) {
        let rec = self.mode(mode);
        let fell_back = rec.map_or(0, |r| r.fell_back.len());
        let spend = rec.map_or(0.0, |r| r.stages.0.value());
        let sel = self.selection(mode).map_or(0.0, |s| s.usd.number());
        (fell_back, spend, sel)
    }
}

/// One row of `judged.json`; the judge's `raw` details are skipped except `metric`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct JudgedLite {
    #[serde(deserialize_with = "present")]
    pub(crate) mode: Option<Loose>,
    #[serde(deserialize_with = "present")]
    pub(crate) arm: Option<Loose>,
    pub(crate) task_id: Loose,
    pub(crate) k: Loose,
    pub(crate) label: Loose,
    pub(crate) run_id: Loose,
    pub(crate) chosen_is_base: Loose,
    pub(crate) n_passed: Loose,
    pub(crate) n_criteria: Loose,
    pub(crate) all_pass: Loose,
    pub(crate) error: Note,
    pub(crate) raw: Obj<RawLite>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct RawLite {
    pub(crate) metric: Loose,
}

/// One row of `base_judged.json`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct BaseLite {
    pub(crate) task: Loose,
    pub(crate) label: Loose,
    pub(crate) run_id: Loose,
    pub(crate) score: Obj<ScoreLite>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ScoreLite {
    pub(crate) n_passed: Loose,
    pub(crate) n_criteria: Loose,
    #[serde(deserialize_with = "present")]
    pub(crate) all_pass: Option<Loose>,
}

/// A sweep record (`_sweeps/<ns>-<stamp>.json`) or `demo.json`, reduced to what the task
/// stream order needs.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct RecordLite {
    /// `demo.json` (one `run_domain` report) has `tasks` at the top.
    #[serde(deserialize_with = "present")]
    tasks: Option<TaskNames>,
    domain_order: Names,
    domains: Domains,
}

impl RecordLite {
    /// Task names in stream order: domains in `domain_order`, then the other domains in
    /// file order, each domain's tasks in file order. A domain name used twice keeps its
    /// first entry (upstream looks domains up by name).
    pub(crate) fn order(self) -> Vec<String> {
        if let Some(tasks) = self.tasks {
            return tasks.0;
        }
        let mut by_name: HashMap<String, Vec<String>> = self.domains.0.iter().cloned().collect();
        let mut order = Vec::new();
        let names = self
            .domain_order
            .0
            .into_iter()
            .chain(self.domains.0.into_iter().map(|(n, _)| n));
        for name in names {
            // Taking the tasks out makes a repeated name a no-op.
            if let Some(tasks) = by_name.remove(&name) {
                order.extend(tasks);
            }
        }
        order
    }
}

/// A list of names (`domain_order`).
#[derive(Debug, Default)]
struct Names(Vec<String>);

struct NamesVisitor;

impl<'de> Visitor<'de> for NamesVisitor {
    type Value = Names;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of names")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Names, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element::<Loose>()? {
            out.extend(v.name());
        }
        Ok(Names(out))
    }

    otherwise_not_seq!(Names, Names::default());
}

impl<'de> Deserialize<'de> for Names {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NamesVisitor)
    }
}

/// `[{"task": name, ...}, ...]` reduced to the non-empty task names.
#[derive(Debug, Default)]
struct TaskNames(Vec<String>);

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct TaskRefLite {
    task: Loose,
}

struct TaskNamesVisitor;

impl<'de> Visitor<'de> for TaskNamesVisitor {
    type Value = TaskNames;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of tasks")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<TaskNames, A::Error> {
        let mut out = Vec::new();
        while let Some(Obj(t)) = seq.next_element::<Obj<TaskRefLite>>()? {
            out.extend(t.task.name().filter(|n| !n.is_empty()));
        }
        Ok(TaskNames(out))
    }

    otherwise_not_seq!(TaskNames, TaskNames::default());
}

impl<'de> Deserialize<'de> for TaskNames {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(TaskNamesVisitor)
    }
}

/// `[{"domain": name, "tasks": [...]}, ...]` as `(name, task names)`, one entry per name
/// (the first), in file order. Domains whose name is not a scalar are left out.
#[derive(Debug, Default)]
struct Domains(Vec<(String, Vec<String>)>);

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct DomainLite {
    domain: Loose,
    tasks: TaskNames,
}

struct DomainsVisitor;

impl<'de> Visitor<'de> for DomainsVisitor {
    type Value = Domains;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of domains")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Domains, A::Error> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        while let Some(Obj(d)) = seq.next_element::<Obj<DomainLite>>()? {
            if let Some(name) = d.domain.name()
                && seen.insert(name.clone())
            {
                out.push((name, d.tasks.0));
            }
        }
        Ok(Domains(out))
    }

    otherwise_not_seq!(Domains, Domains::default());
}

impl<'de> Deserialize<'de> for Domains {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(DomainsVisitor)
    }
}

/// An audit session's `.meta.json`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(crate) struct MetaLite {
    pub(crate) label: Loose,
    pub(crate) ok: Loose,
    pub(crate) channel: Loose,
    pub(crate) model: Loose,
    pub(crate) effort: Loose,
    pub(crate) attempts: Loose,
    pub(crate) wall_s: Loose,
    pub(crate) cost_usd: Loose,
    pub(crate) error: Note,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_and_odd_types_never_fail() {
        let s: SummaryLite = object(
            br#"{"task_id": "a/b", "status": "ok", "errors": ["x", "y"],
                "modes": {"agg": {"ok": 1, "fell_back": ["dom1"], "stages": [{"usd": "0.5", "notes": "long"}, 3, {"usd": true}],
                                  "traceback": "..."},
                          "seq": [1, 2], "other": {}},
                "selections": {"parallel": {"ok": true, "chosen_label": "agg1", "usd": 0.25,
                                            "chosen_tree_is_base": null, "experience": {"cue": "c"}}},
                "extra": {"deep": [1, {"x": [2]}]}}"#,
        )
        .unwrap();
        assert_eq!(s.task_id.text(), "a/b");
        assert_eq!(s.errors.len(), 2);
        assert_eq!(s.modes_present(), vec![Mode::Parallel, Mode::Sequential]);
        let agg = s.mode(Mode::Parallel).unwrap();
        assert!(agg.ok.truthy());
        assert_eq!(s.mode_spend(Mode::Parallel), (1, 1.5, 0.25));
        let sel = s.selection(Mode::Parallel).unwrap();
        assert_eq!(sel.chosen_label.text(), "agg1");
        assert_eq!(sel.chosen_tree_is_base, Some(Loose::Null));
        assert!(!s.mode(Mode::Sequential).unwrap().ok.truthy());
    }

    #[test]
    fn modes_key_wins_over_arms_even_when_null() {
        let s: SummaryLite = object(br#"{"modes": null, "arms": {"agg": {"ok": true}}}"#).unwrap();
        assert!(s.modes_present().is_empty());
        let s: SummaryLite = object(br#"{"arms": {"agg": {"ok": true}}}"#).unwrap();
        assert_eq!(s.modes_present(), vec![Mode::Parallel]);
    }

    /// Same choice as reading the file with `json.loads` and looking modes up by token.
    #[test]
    fn mode_keys_follow_json_loads() {
        let m: ModeMap<Loose> =
            serde_json::from_str(r#"{"parallel": 1, "agg": 2, "parallel": 3, "seq": 4, "seq": 5}"#)
                .unwrap();
        assert_eq!(m.get(Mode::Parallel), Some(&Loose::Num(2.into())));
        assert_eq!(m.get(Mode::Sequential), Some(&Loose::Num(5.into())));
        let m: ModeMap<Loose> = serde_json::from_str(r#"{"sequential": 1}"#).unwrap();
        assert_eq!(m.get(Mode::Sequential), Some(&Loose::Num(1.into())));
    }

    #[test]
    fn documents_that_are_not_objects_are_refused() {
        assert!(object::<SummaryLite>(b"[1, 2, 3]").is_err());
        assert!(object::<SummaryLite>(b"{} trailing").is_err());
        let (none, more) = rows(b"{\"not\": \"a list\"}", 10, |r: Loose| Some(r)).unwrap();
        assert!(none.is_empty() && !more);
        assert!(rows::<Loose, Loose>(b"[1, 2", 10, Some).is_err());
    }

    #[test]
    fn rows_stop_at_the_cap_and_say_so() {
        let keep_numbers = |l: Loose| l.as_f64();
        let (r, more) = rows(b"[1, \"x\", 2, 3]", 3, keep_numbers).unwrap();
        assert_eq!((r, more), (vec![1.0, 2.0, 3.0], false));
        let (r, more) = rows(b"[1, 2, \"x\", 3, 4, {}]", 3, keep_numbers).unwrap();
        assert_eq!((r, more), (vec![1.0, 2.0, 3.0], true));
        // Elements that are not rows past the cap are not an overflow.
        let (r, more) = rows(b"[1, 2, \"x\", null]", 2, keep_numbers).unwrap();
        assert_eq!((r, more), (vec![1.0, 2.0], false));
        // The rest is still checked.
        assert!(rows(b"[1, 2, 3, }", 1, keep_numbers).is_err());
    }

    #[test]
    fn long_strings_are_not_kept() {
        let long = "x".repeat(MAX_FIELD_BYTES + 1);
        let ok = "y".repeat(MAX_FIELD_BYTES);
        let doc = format!(r#"{{"task_id": "{long}", "domain": "{ok}"}}"#);
        let s: SummaryLite = object(doc.as_bytes()).unwrap();
        assert_eq!(s.task_id, Loose::Long(MAX_FIELD_BYTES + 1));
        assert!(s.task_id.truthy() && s.task_id.text().is_empty());
        assert_eq!(s.domain.text(), ok);
        // Messages keep their head, cut on a character boundary.
        let msg = format!("{}é{}", "a".repeat(MAX_NOTE_BYTES - 1), "b".repeat(10));
        let j: JudgedLite = object(format!(r#"{{"error": "{msg}"}}"#).as_bytes()).unwrap();
        assert_eq!(j.error.0.text(), "a".repeat(MAX_NOTE_BYTES - 1));
    }

    #[test]
    fn records_give_the_stream_order() {
        let rec: RecordLite = object(
            br#"{"domain_order": ["b", "a", "b", ["odd"]],
                 "domains": [{"domain": "a", "tasks": [{"task": "a1"}, {"task": null}, 7, {"task": "a2"}]},
                             {"domain": "c", "tasks": [{"task": "c1"}]},
                             {"domain": "b", "tasks": [{"task": "b1"}]},
                             {"domain": "a", "tasks": [{"task": "ignored"}]},
                             {"domain": {"x": 1}, "tasks": [{"task": "nameless"}]}]}"#,
        )
        .unwrap();
        assert_eq!(rec.order(), vec!["b1", "a1", "a2", "c1"]);
        let demo: RecordLite =
            object(br#"{"tasks": [{"task": "t2"}, {"task": "t1"}], "domains": "x"}"#).unwrap();
        assert_eq!(demo.order(), vec!["t2", "t1"]);
    }

    #[test]
    fn loose_follows_python() {
        let (v, _) = rows(
            br#"[null, 0, 2.5, "", "x", [], [1], {}, {"a": 1}, true, "1_000"]"#,
            100,
            |l: Loose| Some(l),
        )
        .unwrap();
        let truth: Vec<bool> = v.iter().map(Loose::truthy).collect();
        assert_eq!(
            truth,
            vec![
                false, false, true, false, true, false, true, false, true, true, true
            ]
        );
        assert_eq!(v[2].number(), 2.5);
        assert_eq!(v[10].number(), 1000.0);
        assert_eq!(v[2].count(), 0);
        assert_eq!(v[6].len(), 1);
    }
}
