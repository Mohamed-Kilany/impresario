//! Full `NSKeyedArchive` decoding for DVT/XCTest payloads.
//!
//! `ns-keyed-archive` resolves the standard Foundation collections but leaves
//! the fields of custom Objective-C objects as `plist::Value::Uid` references.
//! XCTest's identifiers, issues, and activity records are all custom objects,
//! so those fields used to decode as empty. This decoder follows every Uid,
//! including inside custom objects, while preserving the collection shapes the
//! XCTest helper types expect.

use plist::{Dictionary, Value};

use crate::IdeviceError;

type Result<T> = std::result::Result<T, IdeviceError>;

/// Decodes an `NSKeyedArchive` binary plist and resolves all object references.
pub(crate) fn decode(bytes: &[u8]) -> Result<Value> {
    let archive = Value::from_reader(std::io::Cursor::new(bytes)).map_err(invalid)?;
    let dict = archive.as_dictionary().ok_or_else(|| {
        IdeviceError::UnexpectedResponse("NSKeyedArchive root is not a dictionary".into())
    })?;
    let objects = dict
        .get("$objects")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            IdeviceError::UnexpectedResponse("NSKeyedArchive has no $objects array".into())
        })?;
    let root = dict
        .get("$top")
        .and_then(Value::as_dictionary)
        .and_then(|top| top.get("root"))
        .and_then(uid_index)
        .ok_or_else(|| {
            IdeviceError::UnexpectedResponse("NSKeyedArchive has no root object".into())
        })?;

    Decoder { objects }.object(root)
}

fn invalid(error: plist::Error) -> IdeviceError {
    IdeviceError::UnexpectedResponse(format!("decode NSKeyedArchive: {error}"))
}

fn uid_index(value: &Value) -> Option<usize> {
    match value {
        Value::Uid(uid) => Some(uid.get() as usize),
        _ => None,
    }
}

const MAX_DEPTH: usize = 64;

struct Decoder<'a> {
    objects: &'a [Value],
}

impl Decoder<'_> {
    fn object(&self, index: usize) -> Result<Value> {
        self.object_at(index, 0)
    }

    fn object_at(&self, index: usize, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(IdeviceError::UnexpectedResponse(
                "NSKeyedArchive object graph is too deep".into(),
            ));
        }
        let object = self.objects.get(index).ok_or_else(|| {
            IdeviceError::UnexpectedResponse(format!("NSKeyedArchive object {index} is missing"))
        })?;

        match object {
            Value::String(name) if name == "$null" => Ok(Value::Dictionary(Dictionary::new())),
            Value::Uid(uid) => self.object_at(uid.get() as usize, depth + 1),
            Value::Dictionary(dict) => self.dictionary(dict, depth + 1),
            Value::Array(items) => self.array(items, depth + 1),
            other => Ok(other.clone()),
        }
    }

    fn value(&self, value: &Value, depth: usize) -> Result<Value> {
        match value {
            Value::Uid(uid) => self.object_at(uid.get() as usize, depth + 1),
            Value::Dictionary(dict) => self.dictionary(dict, depth + 1),
            Value::Array(items) => self.array(items, depth + 1),
            other => Ok(other.clone()),
        }
    }

    fn array(&self, items: &[Value], depth: usize) -> Result<Value> {
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            values.push(self.value(item, depth)?);
        }
        Ok(Value::Array(values))
    }

    fn dictionary(&self, dict: &Dictionary, depth: usize) -> Result<Value> {
        let class = self.class_name(dict);

        if class.contains("String")
            && let Some(text) = dict.get("NS.string").and_then(Value::as_string)
        {
            return Ok(Value::String(text.to_owned()));
        }
        if class.contains("Data")
            && let Some(data) = dict.get("NS.data").and_then(Value::as_data)
        {
            return Ok(Value::Data(data.to_vec()));
        }
        if class.contains("Dictionary") {
            let keys = dict.get("NS.keys").and_then(Value::as_array);
            let values = dict.get("NS.objects").and_then(Value::as_array);
            let (Some(keys), Some(values)) = (keys, values) else {
                return Ok(Value::Dictionary(Dictionary::new()));
            };
            let mut result = Dictionary::new();
            for (key, value) in keys.iter().zip(values.iter()) {
                let key = self.key(key, depth)?;
                result.insert(key, self.value(value, depth)?);
            }
            return Ok(Value::Dictionary(result));
        }
        if class.contains("Array") || class.contains("Set") {
            let Some(items) = dict.get("NS.objects").and_then(Value::as_array) else {
                return Ok(Value::Array(Vec::new()));
            };
            return self.array(items, depth);
        }

        // Custom Objective-C object: expose its fields, resolve every reference,
        // and drop the class marker that only names the archived class.
        let mut result = Dictionary::new();
        for (key, value) in dict {
            if key == "$class" {
                continue;
            }
            result.insert(key.clone(), self.value(value, depth)?);
        }
        Ok(Value::Dictionary(result))
    }

    fn key(&self, value: &Value, depth: usize) -> Result<String> {
        Ok(match self.value(value, depth)? {
            Value::String(text) => text,
            Value::Integer(number) => number.to_string(),
            Value::Real(number) => number.to_string(),
            other => format!("{other:?}"),
        })
    }

    fn class_name(&self, dict: &Dictionary) -> String {
        dict.get("$class")
            .and_then(uid_index)
            .and_then(|index| self.objects.get(index))
            .and_then(Value::as_dictionary)
            .and_then(|class| class.get("$classname"))
            .and_then(Value::as_string)
            .unwrap_or_default()
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use plist::{Dictionary, Uid, Value};

    use super::decode;

    /// A custom object whose fields point at strings and arrays elsewhere in
    /// the object table, mirroring `XCTTestIdentifier`.
    #[test]
    fn resolves_references_inside_custom_objects() {
        let mut identifier = Dictionary::new();
        identifier.insert("$class".into(), Value::Uid(Uid::new(1)));
        identifier.insert("c".into(), Value::Uid(Uid::new(2)));

        let mut class = Dictionary::new();
        class.insert(
            "$classname".into(),
            Value::String("XCTTestIdentifier".into()),
        );

        let components = Value::Array(vec![
            Value::Uid(Uid::new(4)),
            Value::Uid(Uid::new(5)),
            Value::Uid(Uid::new(6)),
        ]);

        let objects = Value::Array(vec![
            Value::String("$null".into()),
            Value::Dictionary(identifier),
            Value::Dictionary(class),
            components,
            Value::String("ImpresarioAgentTests".into()),
            Value::String("ImpresarioAgentTests".into()),
            Value::String("testServe".into()),
        ]);

        let mut top = Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));

        let mut archive = Dictionary::new();
        archive.insert("$objects".into(), objects);
        archive.insert("$top".into(), Value::Dictionary(top));

        let mut bytes = Vec::new();
        Value::Dictionary(archive)
            .to_writer_binary(&mut bytes)
            .unwrap();
        let decoded = decode(&bytes).unwrap();
        let dict = decoded.as_dictionary().unwrap();
        let components = dict.get("c").and_then(Value::as_array).unwrap();
        let names: Vec<_> = components.iter().filter_map(Value::as_string).collect();
        assert_eq!(
            names,
            ["ImpresarioAgentTests", "ImpresarioAgentTests", "testServe"]
        );
        assert!(!dict.contains_key("$class"));
    }
}
