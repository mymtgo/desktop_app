//! The .NET BinaryFormatter (MS-NRBF) record grammar, as far as MTGO's game
//! history uses it (0.x `ParseGameHistory::parseRecord`): record types 0, 1
//! and 4 to 17. Unlike 0.x, arrays count elements rather than records (a
//! null run is that many elements), so no top-level record is read as an
//! array element.
//!
//! Every record that carries an object id (classes, strings, arrays) is kept
//! in [`Stream::objects`] and appears where it was read as [`Value::Ref`], so
//! references resolve by id after the whole stream is read, forward or back.

use std::collections::HashMap;
use std::sync::Arc;

use crate::HistoryError;

/// How deep records may nest inside each other, and how many references a
/// lookup follows (0.x stopped resolving at depth 50).
pub const MAX_DEPTH: usize = 50;

/// One value read from the stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Every signed integer primitive (and `Byte`).
    Int(i64),
    /// `UInt64`.
    UInt(u64),
    Float(f64),
    /// Strings, chars and decimals (a decimal is written as text).
    Text(String),
    /// The raw 64 bits of a .NET `DateTime`: kind in the top two bits, ticks
    /// since 0001-01-01 below.
    DateTime(u64),
    /// A `TimeSpan` in ticks.
    TimeSpan(i64),
    Object(Object),
    Array(Vec<Value>),
    /// Another record, by object id. Look it up with [`Stream::resolve`].
    Ref(i32),
}

/// A class instance: its class name and member values in declared order.
///
/// The names are shared with the class definition: a `ClassWithId` record
/// takes nine bytes however long its class's names are, so copying them per
/// instance would let a small crafted file allocate gigabytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub class_name: Arc<str>,
    pub members: Vec<(Arc<str>, Value)>,
}

impl Object {
    /// The member's value, if the class has one by that name.
    pub fn member(&self, name: &str) -> Option<&Value> {
        self.members
            .iter()
            .find(|(member, _)| &**member == name)
            .map(|(_, value)| value)
    }
}

/// Where a string's UTF-8 bytes sit in the file (after its length prefix).
/// Fixture tools use this to rewrite strings in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StringSpan {
    pub start: usize,
    pub end: usize,
}

/// A whole stream, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    /// The header's root object id.
    pub root_id: i32,
    /// Every record with an object id.
    pub objects: HashMap<i32, Value>,
    /// Every string payload in the file, in file order: string records,
    /// string primitives and decimals. Class, member and library names are
    /// metadata and are not listed.
    pub strings: Vec<StringSpan>,
}

impl Stream {
    /// The root object's value.
    pub fn root(&self) -> &Value {
        self.lookup(self.root_id)
    }

    /// The value of the record with object id `id`, references followed as
    /// [`Stream::resolve`] does.
    pub fn lookup(&self, id: i32) -> &Value {
        match self.objects.get(&id) {
            Some(value) => self.resolve(value),
            None => &NULL,
        }
    }

    /// The value behind a reference, following at most [`MAX_DEPTH`]
    /// references. A reference to an id the stream never defined (or a chain
    /// too long to follow) is [`Value::Null`].
    pub fn resolve<'s>(&'s self, value: &'s Value) -> &'s Value {
        let mut current = value;
        for _ in 0..MAX_DEPTH {
            match current {
                Value::Ref(id) => match self.objects.get(id) {
                    Some(next) => current = next,
                    None => return &NULL,
                },
                other => return other,
            }
        }
        &NULL
    }
}

static NULL: Value = Value::Null;

/// Arrays may claim at least this many elements, however few bytes are
/// left: a run of nulls takes two bytes for up to 255 elements. The whole
/// stream's arrays together may hold at most the file's length plus this
/// many elements, so a small crafted file cannot expand to gigabytes (MTGO's
/// real file of 273 KB holds under 20,000).
const MIN_ARRAY_CAP: usize = 65_536;

/// Read a whole stream: the header, records up to the message end record.
/// Bytes after the message end are ignored.
pub fn read_stream(bytes: &[u8]) -> Result<Stream, HistoryError> {
    if bytes.is_empty() {
        return Err(HistoryError::Empty);
    }
    if bytes[0] != RECORD_HEADER {
        return Err(HistoryError::BadHeader);
    }
    let mut reader = Reader {
        bytes,
        pos: 1,
        depth: 0,
        classes: HashMap::new(),
        objects: HashMap::new(),
        strings: Vec::new(),
        elements_left: bytes.len().saturating_add(MIN_ARRAY_CAP),
    };
    let root_id = reader.i32()?;
    let _header_id = reader.i32()?;
    let major = reader.i32()?;
    let minor = reader.i32()?;
    if major != 1 || minor != 0 {
        return Err(HistoryError::BadHeader);
    }

    loop {
        let at = reader.pos;
        match reader.record()? {
            Item::End => break,
            Item::Nulls(_) => return Err(HistoryError::BadNullRun { at }),
            Item::Value(_) => {}
        }
    }

    if !reader.objects.contains_key(&root_id) {
        return Err(HistoryError::MissingRoot);
    }
    Ok(Stream {
        root_id,
        objects: reader.objects,
        strings: reader.strings,
    })
}

// Record types (MS-NRBF 2.1.2.1).
const RECORD_HEADER: u8 = 0;
const RECORD_CLASS_WITH_ID: u8 = 1;
const RECORD_SYSTEM_CLASS_WITH_MEMBERS_AND_TYPES: u8 = 4;
const RECORD_CLASS_WITH_MEMBERS_AND_TYPES: u8 = 5;
const RECORD_BINARY_OBJECT_STRING: u8 = 6;
const RECORD_BINARY_ARRAY: u8 = 7;
const RECORD_MEMBER_PRIMITIVE_TYPED: u8 = 8;
const RECORD_MEMBER_REFERENCE: u8 = 9;
const RECORD_OBJECT_NULL: u8 = 10;
const RECORD_MESSAGE_END: u8 = 11;
const RECORD_BINARY_LIBRARY: u8 = 12;
const RECORD_OBJECT_NULL_MULTIPLE_256: u8 = 13;
const RECORD_OBJECT_NULL_MULTIPLE: u8 = 14;
const RECORD_ARRAY_SINGLE_PRIMITIVE: u8 = 15;
const RECORD_ARRAY_SINGLE_OBJECT: u8 = 16;
const RECORD_ARRAY_SINGLE_STRING: u8 = 17;

// Member types (MS-NRBF 2.1.2.2).
const MEMBER_PRIMITIVE: u8 = 0;
const MEMBER_STRING: u8 = 1;
const MEMBER_OBJECT: u8 = 2;
const MEMBER_SYSTEM_CLASS: u8 = 3;
const MEMBER_CLASS: u8 = 4;
const MEMBER_OBJECT_ARRAY: u8 = 5;
const MEMBER_STRING_ARRAY: u8 = 6;
const MEMBER_PRIMITIVE_ARRAY: u8 = 7;

// Primitive types (MS-NRBF 2.1.2.3).
const PRIMITIVE_BOOLEAN: u8 = 1;
const PRIMITIVE_BYTE: u8 = 2;
const PRIMITIVE_CHAR: u8 = 3;
const PRIMITIVE_DECIMAL: u8 = 5;
const PRIMITIVE_DOUBLE: u8 = 6;
const PRIMITIVE_INT16: u8 = 7;
const PRIMITIVE_INT32: u8 = 8;
const PRIMITIVE_INT64: u8 = 9;
const PRIMITIVE_SBYTE: u8 = 10;
const PRIMITIVE_SINGLE: u8 = 11;
const PRIMITIVE_TIMESPAN: u8 = 12;
const PRIMITIVE_DATETIME: u8 = 13;
const PRIMITIVE_UINT16: u8 = 14;
const PRIMITIVE_UINT32: u8 = 15;
const PRIMITIVE_UINT64: u8 = 16;
const PRIMITIVE_STRING: u8 = 18;

/// How a member's value is written: a bare primitive, or a record.
#[derive(Debug, Clone, Copy)]
enum MemberKind {
    Primitive(u8),
    Record,
}

#[derive(Debug, Clone)]
struct ClassDef {
    name: Arc<str>,
    members: Vec<(Arc<str>, MemberKind)>,
}

/// What one record gave.
enum Item {
    Value(Value),
    /// `ObjectNullMultiple`: this many null array elements.
    Nulls(usize),
    End,
}

struct Reader<'b> {
    bytes: &'b [u8],
    pos: usize,
    depth: usize,
    classes: HashMap<i32, Arc<ClassDef>>,
    objects: HashMap<i32, Value>,
    strings: Vec<StringSpan>,
    /// How many more array elements the stream may hold in total.
    elements_left: usize,
}

impl Reader<'_> {
    /// Count `length` array elements against the stream-wide total, before
    /// any are read.
    fn claim_elements(&mut self, length: usize, at: usize) -> Result<(), HistoryError> {
        if length > self.elements_left {
            return Err(HistoryError::BadLength { at });
        }
        self.elements_left -= length;
        Ok(())
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, count: usize) -> Result<&[u8], HistoryError> {
        if count > self.remaining() {
            return Err(HistoryError::Truncated { at: self.pos });
        }
        let start = self.pos;
        self.pos += count;
        Ok(&self.bytes[start..self.pos])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], HistoryError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, HistoryError> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, HistoryError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, HistoryError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, HistoryError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// A count that must not be negative.
    fn count(&mut self) -> Result<usize, HistoryError> {
        let at = self.pos;
        let value = self.i32()?;
        usize::try_from(value).map_err(|_| HistoryError::BadLength { at })
    }

    /// A 7-bit encoded length prefix (at most five bytes, at most
    /// `i32::MAX`) and that many UTF-8 bytes.
    fn string_span(&mut self) -> Result<StringSpan, HistoryError> {
        let at = self.pos;
        let mut length: u64 = 0;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            length |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return Err(HistoryError::BadLength { at });
            }
        }
        if length > i32::MAX as u64 {
            return Err(HistoryError::BadLength { at });
        }
        let start = self.pos;
        self.take(length as usize)?;
        Ok(StringSpan {
            start,
            end: self.pos,
        })
    }

    fn text(&self, span: StringSpan) -> String {
        String::from_utf8_lossy(&self.bytes[span.start..span.end]).into_owned()
    }

    /// A string the stream holds as data (listed in [`Stream::strings`]).
    fn data_string(&mut self) -> Result<String, HistoryError> {
        let span = self.string_span()?;
        self.strings.push(span);
        Ok(self.text(span))
    }

    /// A class, member or library name.
    fn metadata_string(&mut self) -> Result<String, HistoryError> {
        let span = self.string_span()?;
        Ok(self.text(span))
    }

    /// The smallest number of bytes one primitive of this type takes.
    fn primitive_size(primitive: u8, at: usize) -> Result<usize, HistoryError> {
        match primitive {
            PRIMITIVE_BOOLEAN | PRIMITIVE_BYTE | PRIMITIVE_CHAR | PRIMITIVE_DECIMAL
            | PRIMITIVE_SBYTE | PRIMITIVE_STRING => Ok(1),
            PRIMITIVE_INT16 | PRIMITIVE_UINT16 => Ok(2),
            PRIMITIVE_INT32 | PRIMITIVE_UINT32 | PRIMITIVE_SINGLE => Ok(4),
            PRIMITIVE_DOUBLE | PRIMITIVE_INT64 | PRIMITIVE_TIMESPAN | PRIMITIVE_DATETIME
            | PRIMITIVE_UINT64 => Ok(8),
            other => Err(HistoryError::UnknownPrimitive {
                primitive_type: other,
                at,
            }),
        }
    }

    fn primitive(&mut self, primitive: u8) -> Result<Value, HistoryError> {
        let at = self.pos;
        let value = match primitive {
            PRIMITIVE_BOOLEAN => Value::Bool(self.u8()? != 0),
            PRIMITIVE_BYTE => Value::Int(i64::from(self.u8()?)),
            PRIMITIVE_CHAR => {
                let lead = *self
                    .bytes
                    .get(self.pos)
                    .ok_or(HistoryError::Truncated { at })?;
                let width = if lead < 0x80 {
                    1
                } else if lead < 0xE0 {
                    2
                } else if lead < 0xF0 {
                    3
                } else {
                    4
                };
                Value::Text(String::from_utf8_lossy(self.take(width)?).into_owned())
            }
            PRIMITIVE_DECIMAL | PRIMITIVE_STRING => Value::Text(self.data_string()?),
            PRIMITIVE_DOUBLE => Value::Float(f64::from_le_bytes(self.array()?)),
            PRIMITIVE_INT16 => Value::Int(i64::from(i16::from_le_bytes(self.array()?))),
            PRIMITIVE_INT32 => Value::Int(i64::from(self.i32()?)),
            PRIMITIVE_INT64 => Value::Int(self.i64()?),
            PRIMITIVE_SBYTE => Value::Int(i64::from(i8::from_le_bytes(self.array()?))),
            PRIMITIVE_SINGLE => Value::Float(f64::from(f32::from_le_bytes(self.array()?))),
            PRIMITIVE_TIMESPAN => Value::TimeSpan(self.i64()?),
            PRIMITIVE_DATETIME => Value::DateTime(self.u64()?),
            PRIMITIVE_UINT16 => Value::Int(i64::from(u16::from_le_bytes(self.array()?))),
            PRIMITIVE_UINT32 => Value::Int(i64::from(u32::from_le_bytes(self.array()?))),
            PRIMITIVE_UINT64 => Value::UInt(self.u64()?),
            other => {
                return Err(HistoryError::UnknownPrimitive {
                    primitive_type: other,
                    at,
                });
            }
        };
        Ok(value)
    }

    /// A member type code's additional info, read after all the codes.
    fn member_kind(&mut self, code: u8, at: usize) -> Result<MemberKind, HistoryError> {
        match code {
            MEMBER_PRIMITIVE => {
                let primitive_at = self.pos;
                let primitive = self.u8()?;
                Self::primitive_size(primitive, primitive_at)?;
                Ok(MemberKind::Primitive(primitive))
            }
            MEMBER_PRIMITIVE_ARRAY => {
                self.u8()?;
                Ok(MemberKind::Record)
            }
            MEMBER_SYSTEM_CLASS => {
                self.metadata_string()?;
                Ok(MemberKind::Record)
            }
            MEMBER_CLASS => {
                self.metadata_string()?;
                self.i32()?;
                Ok(MemberKind::Record)
            }
            MEMBER_STRING | MEMBER_OBJECT | MEMBER_OBJECT_ARRAY | MEMBER_STRING_ARRAY => {
                Ok(MemberKind::Record)
            }
            other => Err(HistoryError::UnknownMemberType {
                member_type: other,
                at,
            }),
        }
    }

    /// One record, with nesting tracked.
    fn record(&mut self) -> Result<Item, HistoryError> {
        let at = self.pos;
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(HistoryError::TooDeep { at });
        }
        let item = self.record_body(at);
        self.depth -= 1;
        item
    }

    fn record_body(&mut self, at: usize) -> Result<Item, HistoryError> {
        let record_type = self.u8()?;
        let value = match record_type {
            RECORD_CLASS_WITH_ID => {
                let id = self.i32()?;
                let metadata_at = self.pos;
                let metadata_id = self.i32()?;
                let class = self.classes.get(&metadata_id).map(Arc::clone).ok_or(
                    HistoryError::UnknownClass {
                        metadata_id,
                        at: metadata_at,
                    },
                )?;
                self.class_values(id, &class)?
            }
            RECORD_SYSTEM_CLASS_WITH_MEMBERS_AND_TYPES | RECORD_CLASS_WITH_MEMBERS_AND_TYPES => {
                self.class_with_members(record_type == RECORD_CLASS_WITH_MEMBERS_AND_TYPES)?
            }
            RECORD_BINARY_OBJECT_STRING => {
                let id = self.i32()?;
                let text = self.data_string()?;
                self.objects.insert(id, Value::Text(text));
                Value::Ref(id)
            }
            RECORD_BINARY_ARRAY => self.binary_array()?,
            RECORD_MEMBER_PRIMITIVE_TYPED => {
                let primitive = self.u8()?;
                self.primitive(primitive)?
            }
            RECORD_MEMBER_REFERENCE => Value::Ref(self.i32()?),
            RECORD_OBJECT_NULL => Value::Null,
            RECORD_MESSAGE_END => return Ok(Item::End),
            RECORD_BINARY_LIBRARY => {
                // A library precedes the record that uses it; that record is
                // the value.
                self.i32()?;
                self.metadata_string()?;
                return self.record();
            }
            RECORD_OBJECT_NULL_MULTIPLE_256 => return Ok(Item::Nulls(usize::from(self.u8()?))),
            RECORD_OBJECT_NULL_MULTIPLE => return Ok(Item::Nulls(self.count()?)),
            RECORD_ARRAY_SINGLE_PRIMITIVE => {
                let id = self.i32()?;
                let length = self.count()?;
                let primitive_at = self.pos;
                let primitive = self.u8()?;
                let items = self.primitives(primitive, length, primitive_at)?;
                self.objects.insert(id, Value::Array(items));
                Value::Ref(id)
            }
            RECORD_ARRAY_SINGLE_OBJECT | RECORD_ARRAY_SINGLE_STRING => {
                let id = self.i32()?;
                let length_at = self.pos;
                let length = self.count()?;
                let items = self.elements(length, length_at)?;
                self.objects.insert(id, Value::Array(items));
                Value::Ref(id)
            }
            other => {
                return Err(HistoryError::UnknownRecord {
                    record_type: other,
                    at,
                });
            }
        };
        Ok(Item::Value(value))
    }

    /// A record where a single value belongs (a member, not an array slot).
    fn value_record(&mut self) -> Result<Value, HistoryError> {
        let at = self.pos;
        match self.record()? {
            Item::Value(value) => Ok(value),
            Item::Nulls(_) => Err(HistoryError::BadNullRun { at }),
            Item::End => Err(HistoryError::Truncated { at }),
        }
    }

    /// `ClassWithMembersAndTypes` (with a library id) or
    /// `SystemClassWithMembersAndTypes`.
    fn class_with_members(&mut self, has_library: bool) -> Result<Value, HistoryError> {
        let id = self.i32()?;
        let name = self.metadata_string()?;
        let count_at = self.pos;
        let count = self.count()?;
        // Every member name takes at least a byte.
        if count > self.remaining() {
            return Err(HistoryError::BadLength { at: count_at });
        }
        let mut names = Vec::new();
        for _ in 0..count {
            names.push(Arc::<str>::from(self.metadata_string()?));
        }
        let mut codes = Vec::new();
        for _ in 0..count {
            let code_at = self.pos;
            codes.push((self.u8()?, code_at));
        }
        let mut members = Vec::new();
        for (member, (code, code_at)) in names.into_iter().zip(codes) {
            members.push((member, self.member_kind(code, code_at)?));
        }
        if has_library {
            self.i32()?;
        }
        let class = Arc::new(ClassDef {
            name: Arc::from(name),
            members,
        });
        // Registered before the values: a member may be an instance of the
        // same class.
        self.classes.insert(id, Arc::clone(&class));
        self.class_values(id, &class)
    }

    /// The member values of an instance of `class`, kept as object `id`.
    fn class_values(&mut self, id: i32, class: &ClassDef) -> Result<Value, HistoryError> {
        let mut members = Vec::new();
        for (member, kind) in &class.members {
            let value = match kind {
                MemberKind::Primitive(primitive) => self.primitive(*primitive)?,
                MemberKind::Record => self.value_record()?,
            };
            members.push((Arc::clone(member), value));
        }
        self.objects.insert(
            id,
            Value::Object(Object {
                class_name: Arc::clone(&class.name),
                members,
            }),
        );
        Ok(Value::Ref(id))
    }

    /// `length` bare primitives.
    fn primitives(
        &mut self,
        primitive: u8,
        length: usize,
        at: usize,
    ) -> Result<Vec<Value>, HistoryError> {
        let size = Self::primitive_size(primitive, at)?;
        if length
            .checked_mul(size)
            .is_none_or(|bytes| bytes > self.remaining())
        {
            return Err(HistoryError::BadLength { at });
        }
        self.claim_elements(length, at)?;
        let mut items = Vec::new();
        for _ in 0..length {
            items.push(self.primitive(primitive)?);
        }
        Ok(items)
    }

    /// `length` array elements written as records, null runs expanded.
    fn elements(&mut self, length: usize, at: usize) -> Result<Vec<Value>, HistoryError> {
        if length > self.remaining().max(MIN_ARRAY_CAP) {
            return Err(HistoryError::BadLength { at });
        }
        self.claim_elements(length, at)?;
        let mut items = Vec::new();
        while items.len() < length {
            let record_at = self.pos;
            match self.record()? {
                Item::Value(value) => items.push(value),
                Item::Nulls(count) => {
                    if count > length - items.len() {
                        return Err(HistoryError::BadNullRun { at: record_at });
                    }
                    items.resize(items.len() + count, Value::Null);
                }
                Item::End => return Err(HistoryError::Truncated { at: record_at }),
            }
        }
        Ok(items)
    }

    /// `BinaryArray`: any rank, optional lower bounds, elements of any
    /// member type, flattened.
    fn binary_array(&mut self) -> Result<Value, HistoryError> {
        let id = self.i32()?;
        let array_type_at = self.pos;
        let array_type = self.u8()?;
        // Single, jagged, rectangular, and each with offsets: 0 to 5.
        if array_type > 5 {
            return Err(HistoryError::UnknownArrayType {
                array_type,
                at: array_type_at,
            });
        }
        let rank_at = self.pos;
        let rank = self.count()?;
        if rank > 32 {
            return Err(HistoryError::BadLength { at: rank_at });
        }
        let mut total: usize = 1;
        for _ in 0..rank {
            let length_at = self.pos;
            let length = self.count()?;
            total = total
                .checked_mul(length)
                .ok_or(HistoryError::BadLength { at: length_at })?;
        }
        // Single, jagged and rectangular arrays with offsets carry lower
        // bounds.
        if array_type >= 3 {
            for _ in 0..rank {
                self.i32()?;
            }
        }
        let code_at = self.pos;
        let code = self.u8()?;
        let items = match self.member_kind(code, code_at)? {
            MemberKind::Primitive(primitive) => self.primitives(primitive, total, rank_at)?,
            MemberKind::Record => self.elements(total, rank_at)?,
        };
        self.objects.insert(id, Value::Array(items));
        Ok(Value::Ref(id))
    }
}

/// Builders for hand-made streams in tests.
#[cfg(test)]
pub(crate) mod build {
    /// A member's type in a class definition record.
    #[derive(Clone, Copy)]
    pub enum Member {
        Primitive(u8),
        String,
        Object,
        SystemClass(&'static str),
        Class(&'static str, i32),
        ObjectArray,
        StringArray,
        PrimitiveArray(u8),
    }

    pub const BOOL: u8 = 1;
    pub const INT32: u8 = 8;
    pub const DATETIME: u8 = 13;
    pub const STRING: u8 = 18;

    /// Bytes of a stream under construction.
    pub struct Bytes(pub Vec<u8>);

    impl Bytes {
        /// A serialization header naming `root_id`.
        pub fn header(root_id: i32) -> Bytes {
            let mut bytes = Bytes(Vec::new());
            bytes.u8(0).i32(root_id).i32(-1).i32(1).i32(0);
            bytes
        }

        pub fn u8(&mut self, value: u8) -> &mut Self {
            self.0.push(value);
            self
        }

        pub fn i32(&mut self, value: i32) -> &mut Self {
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }

        pub fn u64(&mut self, value: u64) -> &mut Self {
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }

        /// A 7-bit length prefix and the UTF-8 bytes.
        pub fn str(&mut self, value: &str) -> &mut Self {
            let mut len = value.len();
            loop {
                let byte = (len & 0x7F) as u8;
                len >>= 7;
                if len == 0 {
                    self.0.push(byte);
                    break;
                }
                self.0.push(byte | 0x80);
            }
            self.0.extend_from_slice(value.as_bytes());
            self
        }

        pub fn object_string(&mut self, id: i32, value: &str) -> &mut Self {
            self.u8(6).i32(id).str(value)
        }

        pub fn reference(&mut self, id: i32) -> &mut Self {
            self.u8(9).i32(id)
        }

        pub fn null(&mut self) -> &mut Self {
            self.u8(10)
        }

        pub fn nulls_256(&mut self, count: u8) -> &mut Self {
            self.u8(13).u8(count)
        }

        pub fn nulls(&mut self, count: i32) -> &mut Self {
            self.u8(14).i32(count)
        }

        pub fn library(&mut self, id: i32, name: &str) -> &mut Self {
            self.u8(12).i32(id).str(name)
        }

        /// The start of a class definition record: `record` 4 (system
        /// class) or 5 (class, with `library`). Member values follow.
        pub fn class_def(
            &mut self,
            record: u8,
            id: i32,
            name: &str,
            members: &[(&str, Member)],
            library: i32,
        ) -> &mut Self {
            self.u8(record).i32(id).str(name).i32(members.len() as i32);
            for (member, _) in members {
                self.str(member);
            }
            for (_, kind) in members {
                let code = match kind {
                    Member::Primitive(_) => 0,
                    Member::String => 1,
                    Member::Object => 2,
                    Member::SystemClass(_) => 3,
                    Member::Class(_, _) => 4,
                    Member::ObjectArray => 5,
                    Member::StringArray => 6,
                    Member::PrimitiveArray(_) => 7,
                };
                self.u8(code);
            }
            for (_, kind) in members {
                match kind {
                    Member::Primitive(primitive) | Member::PrimitiveArray(primitive) => {
                        self.u8(*primitive);
                    }
                    Member::SystemClass(class) => {
                        self.str(class);
                    }
                    Member::Class(class, class_library) => {
                        self.str(class).i32(*class_library);
                    }
                    _ => {}
                }
            }
            if record == 5 {
                self.i32(library);
            }
            self
        }

        pub fn class_with_id(&mut self, id: i32, metadata_id: i32) -> &mut Self {
            self.u8(1).i32(id).i32(metadata_id)
        }

        pub fn int32_array(&mut self, id: i32, values: &[i32]) -> &mut Self {
            self.u8(15).i32(id).i32(values.len() as i32).u8(INT32);
            for value in values {
                self.i32(*value);
            }
            self
        }

        /// An `ArraySingleString` header; `length` elements follow.
        pub fn string_array(&mut self, id: i32, length: i32) -> &mut Self {
            self.u8(17).i32(id).i32(length)
        }

        /// An `ArraySingleObject` header; `length` elements follow.
        pub fn object_array(&mut self, id: i32, length: i32) -> &mut Self {
            self.u8(16).i32(id).i32(length)
        }

        /// A single-dimension `BinaryArray` of class instances; `length`
        /// elements follow.
        pub fn class_array(
            &mut self,
            id: i32,
            length: i32,
            class: &str,
            library: i32,
        ) -> &mut Self {
            self.u8(7)
                .i32(id)
                .u8(0)
                .i32(1)
                .i32(length)
                .u8(4)
                .str(class)
                .i32(library)
        }

        /// The message end record, and the finished bytes.
        pub fn end(&mut self) -> Vec<u8> {
            self.u8(11);
            self.0.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build::{BOOL, Bytes, DATETIME, INT32, Member, STRING};
    use super::*;

    fn text(value: &str) -> Value {
        Value::Text(value.to_string())
    }

    fn object(stream: &Stream, id: i32) -> &Object {
        match stream.lookup(id) {
            Value::Object(object) => object,
            other => panic!("object {id} is {other:?}"),
        }
    }

    fn array(stream: &Stream, id: i32) -> &Vec<Value> {
        match stream.lookup(id) {
            Value::Array(items) => items,
            other => panic!("array {id} is {other:?}"),
        }
    }

    /// A root string and nothing else.
    fn tiny() -> Vec<u8> {
        Bytes::header(1).object_string(1, "hello").end()
    }

    #[test]
    fn reads_a_string_root() {
        let stream = read_stream(&tiny()).expect("reads");
        assert_eq!(stream.root(), &text("hello"));
        assert_eq!(stream.strings.len(), 1);
        let span = stream.strings[0];
        assert_eq!(&tiny()[span.start..span.end], b"hello");
    }

    #[test]
    fn an_empty_file_is_an_error() {
        assert_eq!(read_stream(&[]), Err(HistoryError::Empty));
    }

    #[test]
    fn a_stream_without_a_header_is_an_error() {
        let bytes = [6, 1, 0, 0, 0, 1, b'x', 11];
        assert_eq!(read_stream(&bytes), Err(HistoryError::BadHeader));
    }

    #[test]
    fn an_unknown_record_type_is_an_error() {
        let bytes = Bytes::header(1).u8(0x30).end();
        assert_eq!(
            read_stream(&bytes),
            Err(HistoryError::UnknownRecord {
                record_type: 0x30,
                at: 17
            })
        );
        // Types 2 and 3 (classes without member types) are not supported.
        let bytes = Bytes::header(1).u8(2).end();
        assert!(matches!(
            read_stream(&bytes),
            Err(HistoryError::UnknownRecord { record_type: 2, .. })
        ));
    }

    #[test]
    fn a_missing_message_end_is_truncation() {
        let mut bytes = tiny();
        bytes.pop();
        assert!(matches!(
            read_stream(&bytes),
            Err(HistoryError::Truncated { .. })
        ));
    }

    #[test]
    fn a_missing_root_is_an_error() {
        let bytes = Bytes::header(7).object_string(1, "x").end();
        assert_eq!(read_stream(&bytes), Err(HistoryError::MissingRoot));
    }

    /// A class with every member kind the reader resolves, behind a library.
    fn class_stream() -> Vec<u8> {
        let mut bytes = Bytes::header(1);
        bytes.library(2, "Lib").class_def(
            5,
            1,
            "Thing",
            &[
                ("Count", Member::Primitive(INT32)),
                ("Flag", Member::Primitive(BOOL)),
                ("When", Member::Primitive(DATETIME)),
                ("Name", Member::String),
                ("Later", Member::String),
                ("Missing", Member::Object),
                ("Boxed", Member::Object),
            ],
            2,
        );
        bytes
            .i32(-5)
            .u8(1)
            .u64(0x4000_0000_0000_0001)
            .object_string(3, "inline")
            .reference(4)
            .null()
            .u8(8)
            .u8(INT32)
            .i32(42);
        bytes.object_string(4, "forward").end()
    }

    #[test]
    fn reads_a_class_with_members_and_types_behind_a_library() {
        let stream = read_stream(&class_stream()).expect("reads");
        let thing = object(&stream, 1);
        assert_eq!(&*thing.class_name, "Thing");
        assert_eq!(thing.member("Count"), Some(&Value::Int(-5)));
        assert_eq!(thing.member("Flag"), Some(&Value::Bool(true)));
        assert_eq!(
            thing.member("When"),
            Some(&Value::DateTime(0x4000_0000_0000_0001))
        );
        let name = thing.member("Name").expect("name");
        assert_eq!(stream.resolve(name), &text("inline"));
        let later = thing.member("Later").expect("later");
        assert_eq!(later, &Value::Ref(4));
        assert_eq!(stream.resolve(later), &text("forward"));
        assert_eq!(thing.member("Missing"), Some(&Value::Null));
        assert_eq!(thing.member("Boxed"), Some(&Value::Int(42)));
        assert_eq!(stream.strings.len(), 2);
    }

    #[test]
    fn reads_a_system_class_and_class_with_id() {
        let mut bytes = Bytes::header(1);
        bytes
            .class_def(
                4,
                1,
                "System.Pair",
                &[("A", Member::Primitive(INT32)), ("B", Member::Object)],
                0,
            )
            .i32(1)
            .class_with_id(2, 1)
            .i32(2)
            .null();
        let stream = read_stream(&bytes.end()).expect("reads");
        let outer = object(&stream, 1);
        assert_eq!(&*outer.class_name, "System.Pair");
        assert_eq!(outer.member("A"), Some(&Value::Int(1)));
        assert_eq!(outer.member("B"), Some(&Value::Ref(2)));
        let inner = object(&stream, 2);
        assert_eq!(&*inner.class_name, "System.Pair");
        assert_eq!(inner.member("A"), Some(&Value::Int(2)));
        assert_eq!(inner.member("B"), Some(&Value::Null));
    }

    #[test]
    fn class_with_id_records_share_one_copy_of_the_class_names() {
        // A crafted stream: a one megabyte class and member name, then
        // twenty thousand ten byte `ClassWithId` records of it. Copying the
        // names per instance would allocate forty gigabytes.
        let long_name = "N".repeat(1 << 20);
        let instances: i32 = 20_000;
        let mut bytes = Bytes::header(1);
        bytes
            .class_def(
                4,
                1,
                &long_name,
                &[(long_name.as_str(), Member::Primitive(BOOL))],
                0,
            )
            .u8(1);
        for id in 2..2 + instances {
            bytes.class_with_id(id, 1).u8(0);
        }
        let stream = read_stream(&bytes.end()).expect("reads");
        let first = object(&stream, 1);
        let last = object(&stream, 1 + instances);
        assert!(Arc::ptr_eq(&first.class_name, &last.class_name));
        assert!(Arc::ptr_eq(&first.members[0].0, &last.members[0].0));
        assert_eq!(last.member(&long_name), Some(&Value::Bool(false)));
    }

    #[test]
    fn class_with_id_needs_a_known_class() {
        let bytes = Bytes::header(1).class_with_id(1, 9).end();
        assert!(matches!(
            read_stream(&bytes),
            Err(HistoryError::UnknownClass { metadata_id: 9, .. })
        ));
    }

    #[test]
    fn reads_primitive_arrays() {
        let mut bytes = Bytes::header(1);
        bytes.int32_array(1, &[3, -4, 5]);
        let stream = read_stream(&bytes.end()).expect("reads");
        assert_eq!(
            array(&stream, 1),
            &vec![Value::Int(3), Value::Int(-4), Value::Int(5)]
        );
    }

    #[test]
    fn string_primitives_are_listed_as_strings() {
        let mut bytes = Bytes::header(1);
        bytes.u8(15).i32(1).i32(2).u8(STRING).str("ab").str("cd");
        let raw = bytes.end();
        let stream = read_stream(&raw).expect("reads");
        assert_eq!(array(&stream, 1), &vec![text("ab"), text("cd")]);
        let spans: Vec<&[u8]> = stream
            .strings
            .iter()
            .map(|span| &raw[span.start..span.end])
            .collect();
        assert_eq!(spans, vec![b"ab".as_slice(), b"cd".as_slice()]);
    }

    #[test]
    fn null_runs_expand_inside_object_arrays() {
        let mut bytes = Bytes::header(1);
        bytes
            .object_array(1, 7)
            .null()
            .nulls_256(2)
            .object_string(2, "a")
            .nulls(3)
            // The next record is top level: a reader counting records would
            // take it as the array's fifth element.
            .object_string(3, "after");
        let stream = read_stream(&bytes.end()).expect("reads");
        assert_eq!(
            array(&stream, 1),
            &vec![
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Ref(2),
                Value::Null,
                Value::Null,
                Value::Null,
            ]
        );
        assert_eq!(stream.lookup(3), &text("after"));
    }

    #[test]
    fn null_runs_expand_inside_string_arrays() {
        let mut bytes = Bytes::header(1);
        bytes
            .string_array(1, 4)
            .object_string(2, "a")
            .nulls_256(2)
            .reference(2)
            .object_string(3, "after");
        let stream = read_stream(&bytes.end()).expect("reads");
        assert_eq!(
            array(&stream, 1),
            &vec![Value::Ref(2), Value::Null, Value::Null, Value::Ref(2)]
        );
        assert_eq!(stream.lookup(3), &text("after"));
    }

    #[test]
    fn binary_arrays_count_elements_not_records() {
        let mut bytes = Bytes::header(1);
        bytes.library(2, "Lib");
        bytes
            .class_array(1, 5, "Item", 2)
            .reference(3)
            .reference(4)
            .nulls_256(3);
        bytes
            .class_def(5, 3, "Item", &[("N", Member::Primitive(INT32))], 2)
            .i32(30);
        bytes.class_with_id(4, 3).i32(40);
        let stream = read_stream(&bytes.end()).expect("reads");
        assert_eq!(
            array(&stream, 1),
            &vec![
                Value::Ref(3),
                Value::Ref(4),
                Value::Null,
                Value::Null,
                Value::Null
            ]
        );
        assert_eq!(object(&stream, 3).member("N"), Some(&Value::Int(30)));
        assert_eq!(object(&stream, 4).member("N"), Some(&Value::Int(40)));
    }

    #[test]
    fn a_binary_array_with_lower_bounds_and_two_dimensions() {
        let mut bytes = Bytes::header(1);
        // Rectangular offset array (type 5), rank 2, lengths 2 x 1, bounds 0 0.
        bytes
            .u8(7)
            .i32(1)
            .u8(5)
            .i32(2)
            .i32(2)
            .i32(1)
            .i32(0)
            .i32(0)
            .u8(0)
            .u8(INT32)
            .i32(7)
            .i32(8);
        let stream = read_stream(&bytes.end()).expect("reads");
        assert_eq!(array(&stream, 1), &vec![Value::Int(7), Value::Int(8)]);
    }

    #[test]
    fn a_null_run_past_the_array_end_is_an_error() {
        let mut bytes = Bytes::header(1);
        bytes.object_array(1, 2).null().nulls_256(2);
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::BadNullRun { .. })
        ));
    }

    #[test]
    fn a_null_run_as_a_member_is_an_error() {
        let mut bytes = Bytes::header(1);
        bytes
            .class_def(4, 1, "T", &[("A", Member::Object)], 0)
            .nulls_256(1);
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::BadNullRun { .. })
        ));
    }

    #[test]
    fn impossible_lengths_are_errors() {
        let mut bytes = Bytes::header(1);
        bytes.object_array(1, -1);
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::BadLength { .. })
        ));
        let mut bytes = Bytes::header(1);
        bytes.object_array(1, i32::MAX).nulls(i32::MAX);
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::BadLength { .. })
        ));
        let mut bytes = Bytes::header(1);
        bytes.u8(15).i32(1).i32(1_000_000).u8(INT32);
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::BadLength { .. })
        ));
        // A string length prefix that never ends.
        let mut bytes = Bytes::header(1);
        bytes
            .u8(6)
            .i32(1)
            .u8(0xFF)
            .u8(0xFF)
            .u8(0xFF)
            .u8(0xFF)
            .u8(0xFF)
            .u8(0x01);
        assert!(read_stream(&bytes.end()).is_err());
    }

    #[test]
    fn repeated_null_run_arrays_hit_the_stream_element_cap() {
        // Each 14-byte pair claims 65,536 elements. A per-array cap alone
        // would let a small file expand to gigabytes.
        let mut bytes = Bytes::header(1);
        bytes.object_string(1, "root");
        for id in 2..102 {
            bytes.object_array(id, 65_536).nulls(65_536);
        }
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::BadLength { .. })
        ));
    }

    #[test]
    fn unknown_binary_array_types_are_errors() {
        let mut bytes = Bytes::header(1);
        bytes
            .u8(7)
            .i32(1)
            .u8(6)
            .i32(1)
            .i32(1)
            .u8(0)
            .u8(INT32)
            .i32(7);
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::UnknownArrayType { array_type: 6, .. })
        ));
    }

    #[test]
    fn deep_nesting_is_an_error() {
        let mut bytes = Bytes::header(1);
        bytes.class_def(4, 1, "Node", &[("Next", Member::Object)], 0);
        for id in 2..400 {
            bytes.class_with_id(id, 1);
        }
        bytes.null();
        assert!(matches!(
            read_stream(&bytes.end()),
            Err(HistoryError::TooDeep { .. })
        ));
    }

    #[test]
    fn a_self_reference_resolves_without_looping() {
        let mut bytes = Bytes::header(1);
        bytes
            .class_def(4, 1, "Node", &[("Next", Member::Object)], 0)
            .reference(1);
        let stream = read_stream(&bytes.end()).expect("reads");
        let node = object(&stream, 1);
        let next = stream.resolve(node.member("Next").expect("next"));
        assert_eq!(next, stream.root());
        assert_eq!(stream.lookup(99), &Value::Null);
    }

    #[test]
    fn every_cut_of_a_valid_stream_is_an_error() {
        for bytes in [class_stream(), tiny()] {
            for cut in 0..bytes.len() {
                assert!(read_stream(&bytes[..cut]).is_err(), "cut at {cut}");
            }
        }
    }
}
