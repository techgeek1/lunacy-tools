use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::{arg, command, value_parser, ArgMatches};
use json::{JsonValue, object};
use tempdir::TempDir;
use uuid::Uuid;

/// A generic error type.
type Error = Box<dyn std::error::Error>;

fn main() {
    // Parse the program matches.
    let matches = command!()
        .arg(
            arg!([FILE] "the lunacy .free file to process")
                .required(true)
                .value_parser(value_parser!(PathBuf))
        )
        .arg(
            arg!(--group <GROUP> "set the group containing the colors to modify, defaults to 'theme' if unspecified")
                .required(false)
                .value_parser(value_parser!(String))
        )
        .arg(
            arg!(--color_scheme <COLOR_SCHEME> "specify a json file containing a color scheme")
                .id("COLOR_SCHEME")
                .value_parser(value_parser!(PathBuf))
        )
        .get_matches();
    
    // Acquire the document to update from the program arguments.
    let Some(path) = matches.get_one::<PathBuf>("FILE") else {
        panic!("expected .free document as first argument");
    };

    // Read out the group to modify, or default to 'theme'.
    let group = matches.get_one::<String>("group")
        .map(|x| x.to_owned())
        .unwrap_or_else(|| String::from("theme"));

    // Parse the color scheme to modify.
    let scheme = load_color_scheme(&matches);

    // Bail with no errors if there are no colors to update.
    if scheme.colors.is_empty() {
        return;
    }

    // Update the document colors.
    let mut doc = LunacyDocument::open(&path)
        .expect("failed to open document");

    doc.update_colors(&group, &scheme)
        .expect("failed to update colors in document");
    doc.commit()
        .expect("failed to commit changes to document");
}

/// Load the color scheme from the program arguments.
fn load_color_scheme(matches: &ArgMatches) -> ColorScheme {
    let mut scheme = ColorScheme { colors: vec![] };

    // Load the JSON schema first if provided.
    if let Some(colors_json) = matches.get_one::<PathBuf>("COLOR_SCHEME") {
        if let Ok(json_str) = std::fs::read_to_string(colors_json) {
            if let Ok(json) = json::parse(&json_str) {
                for (name, color) in json.entries() {
                    // `value` or `link` are required.
                    let value = color["value"].as_str()
                        .or(color["link"].as_str())
                        .expect("expected `link` or `value`");
                    // `stop` is optional and defaults to 500 if not present.
                    let stop  = color.has_key("stop")
                        .then(|| color["stop"].as_u32().unwrap())
                        .unwrap_or(500);

                    scheme.colors.push(BaseColor {
                        name    : name.to_owned(),
                        value   : value.to_owned(),
                        stop    : stop,
                    })
                }
            }
        }
    }

    scheme
}

/// A set of colors defining a color scheme to apply to a Lunacy document.
struct ColorScheme {
    /// A set of base colors to generate a color palette from.
    colors  : Vec<BaseColor>
}

#[derive(Clone)]
struct BaseColor {
    /// The name of the color.
    name    : String,
    /// The hexadecimal value of the color or the name of a color to link to.
    value   : String,
    /// The stop the color starts at.
    stop    : u32,
}

/// The stops to emit for the color.
const STOPS : &'static [u32]
    = &[100, 200, 300, 400, 500, 600, 700, 800, 900];

impl BaseColor {
    /// Create a color from a base color.
    fn create_tints(&self, group: &str) -> Result<Vec<Color>, Error> {
        let (r, g, b)   = hex_to_rgb(&self.value)?;
        let mid         = (STOPS.len() - 1) / 2;
        let max         = 0.8;
        
        let mut tints = Vec::with_capacity(STOPS.len());
        for (i, stop) in STOPS.iter().enumerate() {
            let hex;

            if *stop == self.stop {
                hex = self.value.clone();
            }
            else {
                let t;
                let dst;
    
                if i < mid {
                    t   = (mid - i) as f64 / mid as f64;
                    dst = 1.0;
                }
                else {
                    t   = (i - mid) as f64 / mid as f64;
                    dst = 0.0;
                };
    
                let t       = max * t;
                let new_r   = lerp(r, dst, t);
                let new_g   = lerp(g, dst, t);
                let new_b   = lerp(b, dst, t);
    
                hex         = rgb_to_hex(new_r, new_g, new_b);
            }

            let name_stem = self.name.split('/')
                .last()
                .unwrap()
                .trim();

            tints.push(Color {
                id      : Uuid::new_v4(),
                version : 1,
                name    : format!("{group} / {} / {name_stem}.{stop}", self.name),
                value   : hex
            });
        }
        
        Ok(tints)
    }
}

/// A color palette from a lunacy document.
#[derive(Default)]
struct ColorPalette {
    /// The set of colors in a color palette.
    colors: BTreeMap<String, Color>
}

impl ColorPalette {
    /// Update a color in the palette by name, updating the existing color or creating a new
    /// one if missing.
    fn update_by_name(&mut self, color: Color) {
        if let Some(x) = self.colors.get_mut(&color.name) {
            x.version  += 1;
            x.value     = color.value.clone();
        }
        else {
            self.colors.insert(color.name.to_owned(), color);
        }
    }

    /// Link in a color to an existing color by name.
    fn link_by_name(&mut self, color: &BaseColor, group: &str) {
        let term = format!("{group} / {}", color.value);
        match self.colors.get(&term) {
            None        => panic!("color {} not found in palette", color.value),
            Some(src)   => {
                let color = Color {
                    id      : Uuid::new_v4(),
                    version : 1,
                    name    : format!("{group} / {}", color.name),
                    value   : src.value.clone()
                };

                self.update_by_name(color);
            }
        }
    }
}

/// A tint in a sequence of color tints.
#[derive(Debug)]
struct Color {
    /// The unique id of the color.
    id      : Uuid,
    /// The version of the color.
    version : u32,
    /// The name of the tint.
    name    : String,
    /// The hex value of the color.
    value   : String,
}

impl Color {
    // Create a new color from a json representation, filtering by prefix..
    fn from_json(json: &JsonValue, prefix: &str) -> Option<Color> {
        /// Decode a uuid from a lunacy id.
        fn decode_id(id: &str) -> Uuid {
            let bytes = URL_SAFE_NO_PAD.decode(id).unwrap();
            let uuid  = Uuid::from_slice(&bytes)
                .unwrap();

            uuid
        }

        let name = json["name"].as_str()
            .unwrap();

        if !name.starts_with(prefix) {
            return None;
        }

        let name = name
            .strip_prefix(&prefix)
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .trim();

        Some(Color {
            id      : decode_id(json["id"].as_str().unwrap()),
            version : json["version"].as_u32().unwrap_or(1),
            name    : name.to_owned(),
            value   : json["value"].as_str().unwrap().to_owned()
        })
    }

    /// Format the color as a JSON string.
    fn to_json_obj(&self) -> Result<JsonValue, Error> {
        /// Encode a uuid to a lunacy id.
        fn encode_id(id: &Uuid) -> String {
            URL_SAFE_NO_PAD.encode(id.as_bytes())
        }
        
        Ok(object! {
            "id"        : encode_id(&self.id),
            "version"   : self.version,
            "name"      : self.name.as_str(),
            "value"     : self.value.as_str(),
        })
    }
}

/// A lunacy document opened for edit.
struct LunacyDocument {
    /// The path to the document we're editing.
    doc_path: PathBuf,
    /// The directory containing the extracted document.
    doc_dir : TempDir,
}

impl LunacyDocument {
    /// Open the document at `path` for edit.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        // Ensure the file is a .free file.
        let path = path.as_ref();
        if path.extension().map(|x| x.to_str()).flatten() != Some("free") {
            return Err(Box::new(io::Error::new(io::ErrorKind::Unsupported, "only `.free` files are supported")));
        }

        // Open a temp directory to hold the document contents.
        let dir = tempdir::TempDir::new("lunacy-tools")?;
        // Extract the document to the directory.
        zip_extensions::zip_extract(&path.to_path_buf(), &dir.path().to_owned())?;

        Ok(Self {
            doc_path: path.to_owned(),
            doc_dir : dir
        })
    }

    /// Commit changes to the document.
    pub fn commit(&mut self) -> Result<(), Error>{
        zip_extensions::zip_create_from_directory(
            &self.doc_path,
            &self.doc_dir.path().to_owned(),
        )?;

        Ok(())
    }

    /// Update colors in the document with the provided color scheme.
    pub fn update_colors(&mut self, group: &str, scheme: &ColorScheme) -> Result<(), Error> {
        // Load the document and resolve any existing colors.
        let mut json    = self.load_json("document.json")?;
        let mut palette = Self::parse_color_palette(&json, group);

        eprintln!("generate");

        // Modify or extend the color palette as requested by the user.
        for base_color in scheme.colors.iter() {
            // Values with a hashtag are generative colors.
            if base_color.value.starts_with("#") {
                for color in base_color.create_tints(group)? {
                    palette.update_by_name(color);
                }
            }
            // Otherwise they're link colors.
            else {
                palette.link_by_name(&base_color, group);
            }
        }

        // Apply changes back to the JSON file.
        Self::apply_color_palette(&mut json, &palette, group)?;
        self.save_json("document.json", &json)?;

        Ok(())
    }
}

impl LunacyDocument {
    /// Load a JSON document from the opened lunacy document.
    fn load_json(&self, path: impl AsRef<Path>) -> Result<JsonValue, Error> {
        let document    = self.doc_dir.path().join(path);
        let data        = std::fs::read_to_string(&document)?;
        let json        = json::parse(&data)?;

        Ok(json)
    }

    /// Save a JSON document back to an opened lunacy document.
    fn save_json(&self, path: impl AsRef<Path>, json: &JsonValue) -> Result<(), Error> {
        let document    = self.doc_dir.path().join(path);
        let json_str    = json.to_string();
        
        std::fs::write(document, json_str)?;

        Ok(())
    }

    /// Parse the color palette from a `document.json` file in the specified group.
    fn parse_color_palette(json: &JsonValue, group: &str) -> ColorPalette {
        let color_variables = &json["colorVariables"];
        let mut palette     = ColorPalette::default();

        let prefix          = format!("{group} / ");
        for i in 0..color_variables.len() {
            let color_var   = &color_variables[i];
            let Some(color) = Color::from_json(&color_var, &prefix) else {
                continue;
            };

            // Add the color to the list.
            palette.colors.insert(color.name.clone(), color);
        }

        palette
    }

    /// Apply `palette` to a `document.json` file.
    fn apply_color_palette(
        json    : &mut JsonValue,
        palette : &ColorPalette,
        group   : &str
    )
        -> Result<(), Error>
    {
        eprintln!("apply");
        let JsonValue::Array(color_variables) = &mut json["colorVariables"] else {
            return Ok(());
        };

        eprintln!("remove");

        // Remove the old colors from the variable list.
        for color in palette.colors.values() {
            // Remove any variables that start with our colors.
            let prefix = format!("{group} / {}", color.name);
            let mut i  = 0;
            while i < color_variables.len() {
                if color_variables[i]["name"].as_str().unwrap().starts_with(&prefix) {
                    color_variables.remove(i);
                }
                else {
                    i += 1;
                }
            }
        }

        eprintln!("insert");

        // Now insert the updated colors.
        for color in palette.colors.values() {
            println!("{} -> {}", color.name, color.value);
            color_variables.push(color.to_json_obj()?);
        }

        Ok(())
    }
}

/// Linearly interpolate from a -> b by `t`.
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a * (1.0 - t) + b * t
}

/// Parse a hex value to an RGB tuple.
fn hex_to_rgb(value: &str) -> Result<(f64, f64, f64), Error> {
    // We expect either #RRGGBB format only.
    if value.len() != 7 && value.len() != 9 {
        return Err(Box::new(ColorParseError::InvalidFormat));
    }

    // Make sure the leading hashtag is present.
    if !value.starts_with("#") {
        return Err(Box::new(ColorParseError::InvalidFormat));
    }

    // Parse the hex value.
    let mut hex = u32::from_str_radix(&value[1..], 16)?;

    // Shift over by 8 and add FF to account for the alpha channel
    // in #RRGGBB formats.
    if value.len() != 9 {
        hex <<= 8;
        hex  |= 0xff;
    }

    let b = (hex >> 8  & 0xff)  as f64 / 255.0;
    let g = (hex >> 16  & 0xff) as f64 / 255.0;
    let r = (hex >> 24 & 0xff)  as f64 / 255.0;

    Ok((r, g, b))
}

/// Convert RGB to hex.
fn rgb_to_hex(r: f64, g: f64, b: f64) -> String {
    let r         = (r * 255.0).round() as u32;
    let g         = (g * 255.0).round() as u32;
    let b         = (b * 255.0).round() as u32;
    let value     = (r << 16) | (g << 8) | (b << 0);
    
    format!("{:06x}", value)
}

/// An error raised when parsing a color value from a string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorParseError {
    /// The provided color text was in an unsupported format.
    InvalidFormat,
    /// The provided color value was invalid.
    InvalidValue(std::num::ParseIntError)
}

impl From<std::num::ParseIntError> for ColorParseError {
    fn from(x: std::num::ParseIntError) -> Self {
        Self::InvalidValue(x)
    }
}

impl std::fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::InvalidFormat     => write!(f, "the provided color string was in an unsupported format"),
            Self::InvalidValue(x)   => write!(f, "failed to parse color value - {x}")
        }
    }
}

impl std::error::Error for ColorParseError { }