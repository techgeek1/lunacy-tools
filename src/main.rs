use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::{arg, command, value_parser, ArgMatches};
use json::{JsonValue, object};
use reqwest;
use tempdir::TempDir;
use uuid::Uuid;

const URL: &'static str = "https://www.tints.dev/api";

/// A generic error type.
type Error = Box<dyn std::error::Error>;

fn parse_base_color(s: &str) -> Result<BaseColor, String> {
    let mut segs    = s.split(':');
    let name        = segs.next();
    let value       = segs.next();
    let step        = segs.next();

    match (name, value, step) {
        // At minimium we require a name and a value.
        (Some(name), Some(value), None) => {
            // Ensure the value is a hexadecimal value.
            let _ = u32::from_str_radix(value, 16)
                .map_err(|_| format!("'{value}' is not a hexadecimal color"))?;

            Ok(BaseColor::new(name, value, 500))
        },
        // Explicit step values are also supported.
        (Some(name), Some(value), Some(step)) => {
            // Ensure the value is a hexadecimal value.
            let _ = u32::from_str_radix(value, 16)
                .map_err(|_| format!("'{value}' is not a hexadecimal color"))?;
            let step = u32::from_str_radix(step, 16)
                .map_err(|_| format!("'{step}' is not an unsigned integer"))?;

            Ok(BaseColor::new(name, value, step))
        },
        // Everything else is an error.
        _ => {
            Err(format!("'{s}' is not a valid base color string, colors must be specified in '<name>:<hex>[:<step>]; format"))
        }
    }
}

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
            arg!(--color <COLOR> "specify a color to modify in the color palette, explicit colors always take precedence")
                .id("COLOR")
                .required_unless_present("COLOR_SCHEME")
                .value_parser(parse_base_color)
                .value_terminator(";")
        )
        .arg(
            arg!(--color_scheme <COLOR_SCHEME> "specify a json file containing a color scheme")
                .id("COLOR_SCHEME")
                .required_unless_present("COLOR")
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
                    // `value` is required.
                    let value = color["value"].as_str().unwrap();
                    // `step` is optional and defaults to 500 if not present.
                    let step  = color.has_key("step")
                        .then(|| color["step"].as_u32().unwrap())
                        .unwrap_or(500);

                    scheme.colors.push(BaseColor {
                        name    : name.to_owned(),
                        value   : value.to_owned(),
                        step    : step
                    })
                }
            }
        }
    }

    // Read out the colors from the command line and build a scheme.
    //
    // Command line colors always take precedence.
    if let Some(colors) = matches.get_many::<BaseColor>("COLOR") {
        for color in colors {
            scheme.colors.push(color.clone());
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
    /// The hexadecimal value of the color.
    value   : String,
    /// The step the color starts at.
    step    : u32
}

impl BaseColor {
    /// Create a new base color for a color scheme update.
    fn new(name: &str, value: &str, step: u32) -> Self {
        Self {
            name    : name.to_owned(),
            value   : value.to_owned(),
            step    : step
        }
    }
}

/// A color palette from a lunacy document.
#[derive(Default)]
struct ColorPalette {
    /// The set of colors in a color palette.
    colors  : Vec<ColorTints>,
}

impl ColorPalette {
    /// Find an exiting color in the palette, creating it if missing.
    fn find_or_insert(&mut self, name: &str) -> &mut ColorTints {
        for i in 0..self.colors.len() {
            if self.colors[i].name.as_str() == name {
                return &mut self.colors[i];
            }
        }
        
        let tints = ColorTints {
            name    : name.to_owned(),
            tints   : Vec::new()
        };

        self.colors.push(tints);
        self.colors.last_mut().unwrap()
    }
}

/// A set of tints for a single color in a palette.
#[derive(Default)]
struct ColorTints {
    /// The name of the color.
    name : String,
    /// The various tints for the color.
    tints: Vec<Color>,
}

impl ColorTints {
    /// Add a new color entry 
    fn push(&mut self, color: Color) {
        self.tints.push(color);
    }

    /// Update the colors in `self` by name matching so their ids are preserved.
    fn update_by_name(&mut self, other: &mut Self) {
        for new in other.tints.drain(..) {
            let exact_name = new.name.split('/')
                .last()
                .unwrap();

            let old = self.tints.iter_mut()
                .find(|x| x.name.ends_with(exact_name));

            // If a color exists with the old name we can update the value in-place.
            if let Some(old) = old {
                old.version += 1;
                old.value    = new.value;
            }
            // Otherwise we need to add a new color.
            else {
                self.tints.push(new);
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
    // Create a new color from a json representation.
    fn from_json(json: &JsonValue) -> Color {
        /// Decode a uuid from a lunacy id.
        fn decode_id(id: &str) -> Uuid {
            let bytes = URL_SAFE_NO_PAD.decode(id).unwrap();
            let uuid  = Uuid::from_slice(&bytes)
                .unwrap();

            uuid
        }

        Color {
            id      : decode_id(json["id"].as_str().unwrap()),
            version : json["version"].as_u32().unwrap_or(1),
            name    : json["name"].as_str().unwrap().to_owned(),
            value   : json["value"].as_str().unwrap().to_owned()
        }
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

        // Modify or extend the color palette as requested by the user.
        for base_color in scheme.colors.iter() {
            // TODO: Compute tints locally, there's no reason to call a webapi.

            // Acquire the tints from `tints.dev`.
            let mut new_tints   = query_tints(group, &base_color.name, &base_color.value)?;
            let tints           = palette.find_or_insert(&base_color.name);

            // Update the tints in the palette by name.
            tints.update_by_name(&mut new_tints);
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
            let color       = Color::from_json(&color_var);

            // Skip any colors not in the desired group.
            if !color.name.starts_with(&prefix) {
                continue;
            } 

            // Get the plain color name without the prefix or suffix.
            let color_name = color.name.strip_prefix(&prefix)
                .unwrap()
                .split('/')
                .next()
                .unwrap()
                .trim();

            // Add the color to the list.
            palette.find_or_insert(color_name)
                .push(color);
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
        let JsonValue::Array(color_variables) = &mut json["colorVariables"] else {
            return Ok(());
        };

        // Remove the old colors from the variable list.
        for color in palette.colors.iter() {
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

        // Now insert the updated colors.
        for color in palette.colors.iter() {
            for tint in color.tints.iter() {
                color_variables.push(tint.to_json_obj()?);
            }
        }

        Ok(())
    }
}

/// Query the tints for `name` and `color` at step 500 and return the JSON result.
fn query_tints(group: &str, name: &str, hex: &str) -> Result<ColorTints, Error> {
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

    // Query the API.
    let url         = format!("{URL}/{name}/{hex}");
    let json_str    = reqwest::blocking::get(url)?.text()?;
    let json        = json::parse(&json_str)?;

    // Parse out into a new tints set.
    let mut tints   = Vec::with_capacity(STEPS.len());
    let color       = &json[name];

    for step in STEPS {
        tints.push(Color {
            id      : Uuid::new_v4(),
            version : 1,
            name    : format!("{group} / {name} / {name}.{step}"),
            value   : color[*step].as_str().unwrap().trim_start_matches("#").to_owned()
        });
    }

    Ok(ColorTints { name: name.to_owned(), tints })
}
