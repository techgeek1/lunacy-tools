use std::borrow::Cow;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use json::JsonValue;
use reqwest;
use uuid::Uuid;

const URL: &'static str = "https://www.tints.dev/api";

fn main() {
    // TODO: Combine queries.
    emit_colors("dark", "1D2023")
}

/// Emit Lunacy compatible colors from a `tints.dev` query.
fn emit_colors(name: &str, hex: &str) {
    let json    = query_tints(name, hex);
    let tints   = parse_tints(name, &json);
    
    for tint in tints {
        let uuid    = Uuid::new_v4();
        let id      = encode_id(&uuid);
        let name    = &tint.name;
        let value   = &tint.value;
        let obj     = format!("\
            {{\
                \"id\": \"{id}\",\
                \"version\": 1,\
                \"name\": \"{name}\",\
                \"value\": \"{value}\"\
            }},\
        ");

        println!("{obj}");
    }
}

/// Query the tints for `name` and `color` at step 500 and return the JSON result.
fn query_tints(name: &str, hex: &str) -> JsonValue {
    let url     = format!("{URL}/{name}/{hex}");
    let json    = reqwest::blocking::get(url)
        .expect("request failed")
        .text()
        .expect("failed to get json");
        
    json::parse(&json).unwrap()
}

/// Parse tints from a `tints.dev` json to a usable key value pair.
fn parse_tints<'json>(name: &str, json: &'json JsonValue) -> Vec<Color<'json>> {
    const STEPS: &'static [&'static str] = &[
        "100",
        "200",
        "300",
        "400",
        "500",
        "600",
        "700",
        "800",
        "900",
    ];

    let mut tints   = Vec::with_capacity(STEPS.len());
    let color       = &json[name];

    for step in STEPS {
        tints.push(Color {
            name    : format!("Palette / {name} / {name}.{step}"),
            value   : color[*step].as_str().unwrap().trim_start_matches("#")
        });
    }
    
    tints
}

/// Extract a `.free` file into a temp directory and return the path.
fn extract_file(file: &Path) -> PathBuf {
    todo!()
}

/// A tint in a sequence of color tints.
#[derive(Debug)]
struct Color<'a> {
    /// The unique id of the color.
    id      : Uuid,
    /// The version of the color.
    version : u32,
    /// The name of the tint.
    name    : Cow<'a, str>,
    /// The hex value of the color.
    value   : &'a str,
}

impl<'a> Color<'a> {
    /// Create a new color from a name and hex value.
    fn new(name: impl Into<Cow<'a, str>>, hex: &'a str) -> Self {
        Self {
            id    : Uuid::new_v4(),
            version : 1,
            name    : name.into(),
            value     : hex
        }
    }

    // Create a new color from a json representation.
    fn from_json<'json>(json: &'json JsonValue) -> Color<'json> {
        Self {
            id      : decode_id(json["id"].as_str()),
            version : json["version"].as_u32(),
            name    : json["name"].as_str().into(),
            value   : json["value"].as_str()
        }
    }
}

/// Decode a uuid from a lunacy id.
fn decode_id(id: &str) -> Uuid {
    let bytes = URL_SAFE_NO_PAD.decode(id).unwrap();
    let uuid  = Uuid::from_slice(&bytes)
        .unwrap();

    uuid
}

/// Encode a uuid to a lunacy id.
fn encode_id(id: &Uuid) -> String {
    URL_SAFE_NO_PAD.encode(id.as_bytes())
}