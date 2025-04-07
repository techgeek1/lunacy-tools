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

fn parse_base_color(s: &str) -> Result<BaseColor, String> {
    let mut segs    = s.split(':');
    let name        = segs.next();
    let value       = segs.next();
    let stop        = segs.next();

    match (name, value, stop) {
        // At minimium we require a name and a value.
        (Some(name), Some(value), None) => {
            // Ensure the value is a hexadecimal value.
            let _ = u32::from_str_radix(value, 16)
                .map_err(|_| format!("'{value}' is not a hexadecimal color"))?;

            Ok(BaseColor::new(name, value, 500))
        },
        // Explicit stop values are also supported.
        (Some(name), Some(value), Some(stop)) => {
            // Ensure the value is a hexadecimal value.
            let _ = u32::from_str_radix(value, 16)
                .map_err(|_| format!("'{value}' is not a hexadecimal color"))?;
            let stop = u32::from_str_radix(stop, 16)
                .map_err(|_| format!("'{stop}' is not an unsigned integer"))?;

            Ok(BaseColor::new(name, value, stop))
        },
        // Everything else is an error.
        _ => {
            Err(format!("'{s}' is not a valid base color string, colors must be specified in '<name>:<hex>[:<stop>]; format"))
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
                    // `stop` is optional and defaults to 500 if not present.
                    let stop  = color.has_key("stop")
                        .then(|| color["stop"].as_u32().unwrap())
                        .unwrap_or(500);
                    // `l_min` is optional and defaults to 0 if not present.
                    let l_min = color.has_key("l_min")
                        .then(|| color["l_min"].as_u32().unwrap())
                        .unwrap_or(0);
                    // `l_max` is optional and defaults to 100 if not present.
                    let l_max = color.has_key("l_max")
                        .then(|| color["l_max"].as_u32().unwrap())
                        .unwrap_or(100);

                    scheme.colors.push(BaseColor {
                        name    : name.to_owned(),
                        value   : value.to_owned(),
                        stop    : stop,
                        l_min   : l_min,
                        l_max   : l_max
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
    /// The stop the color starts at.
    stop    : u32,
    /// The minimum lightness value for the base color.
    l_min   : u32,
    /// The maximum lightness value for the base color.
    l_max   : u32,
}

/// The maximum stops we compute.
const MAX_STOPS         : usize
    = 9;
/// The stops used for computing the color distribution.
const DISTRIBUTION_STOPS: [u32; MAX_STOPS + 4]
    = [0, 50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950, 1000];
/// The stops we actually want to emit.
const STOPS             : [u32; MAX_STOPS]
    = [100, 200, 300, 400, 500, 600, 700, 800, 900];

impl BaseColor {
    /// Create a new base color for a color scheme update.
    fn new(name: &str, value: &str, stop: u32) -> Self {
        Self {
            name    : name.to_owned(),
            value   : value.to_owned(),
            stop    : stop,
            l_min   : 0,
            l_max   : 100
        }
    }

    /// Create a color from a base color.
    fn create_tints(&self, group: &str) -> Result<ColorTints, Error> {
        // Logic derived from https://github.com/SimeonGriggs/tints.dev/blob/main/app/lib/createSwatches.ts#L13.
        let (h, s, l)       = hex_to_hsl(&self.value)?;
        let max             = self.l_max as f64;
        let min             = self.l_min as f64;
        let distribution    = self.create_distribution(max, min, l, self.stop);

        let mut tints = Vec::with_capacity(MAX_STOPS);
        for (i, stop) in STOPS.iter().enumerate() {
            let new_h   = h.rem_euclid(360.0);
            let new_s   = s.clamp(0.0, 100.0);
            let new_l   = distribution[i].clamp(0.0, 100.0);

            let new_hex = hsl_to_hex(new_h, new_s, new_l);

            let color   = Color {
                id      : Uuid::new_v4(),
                version : 1,
                name    : format!("{group} / {} / {}.{stop}", self.name, self.name),
                value   : new_hex
            };

            tints.push(color);
        }
        
        Ok(ColorTints { name: self.name.clone(), tints })
    }

    /// Create a lightness distribution for the base color.
    fn create_distribution(&self, min: f64, max: f64, lightness: f64, stop: u32) -> Vec<f64> {
        /// Find the indenx of `stop` in `stops`.
        fn index_of(stop: u32) -> f64 {
            DISTRIBUTION_STOPS.iter()
                .position(|x| *x == stop)
                .map(|x| x as f64)
                .unwrap_or(-1.0)
        }

        let mut stops   = Vec::with_capacity(MAX_STOPS + 4);
        let mut tweaks  = Vec::with_capacity(MAX_STOPS + 4);
        stops.push(0);      tweaks.push(max);
        stops.push(stop);   tweaks.push(lightness);
        stops.push(1000);   tweaks.push(min);

        for i in 0..DISTRIBUTION_STOPS.len() {
            let stop_value = DISTRIBUTION_STOPS[i];

            // Skip any stops we don't care about. We can't remove them from the array 
            // because it breaks the math but we can enforce that they don't end up in
            // the output.
            match stop_value {
                0 | 1000        => continue,
                x if x == stop  => continue,
                _               => ()
            }
            
            let diff    = ((stop_value as f64 - stop as f64) / 100.0).abs();

            let total;
            let increment;
            let tweak;
            
            if stop_value < stop {
                total       = (index_of(stop) - index_of(DISTRIBUTION_STOPS[0])).abs() - 1.0;
                increment   = max - lightness;
                tweak       = (increment / total) * diff + lightness;
            }
            else {
                total       = (index_of(stop) - index_of(DISTRIBUTION_STOPS[DISTRIBUTION_STOPS.len() - 1])).abs() - 1.0;
                increment   = lightness - min;
                tweak       = lightness - (increment / total) * diff;
            }

            stops.push(stop_value);
            tweaks.push(tweak.round());
        }

        let mut indices = (0..stops.len()).collect::<Vec<_>>();
        indices.sort_by_key(|i| stops[*i]);
        indices.reverse();
        sort_by_indices(tweaks.len(), &mut indices, |a, b| tweaks.swap(a, b));
        
        // Remove 0 and 50, we don't care about them but need them for the algorithm
        // to work correctly.
        tweaks.remove(0);
        tweaks.remove(0);
        // Likewise remove 950 and 100, we don't need them either.
        tweaks.pop();
        tweaks.pop();
        tweaks
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
            // Acquire the tints from `tints.dev`.
            let mut new_tints   = base_color.create_tints(group)?;
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

/// Reorder an external sequence given a sorted vector of `indices` representing the current 
/// positions of each element in the input.
/// 
/// For example, to sort a vector of elements by an external key.
/// ```
/// # use core_algo::sort_by_indices;
/// let mut data    = ["a", "c", "b", "d"];
/// let mut indices = {
///     let mut indices = (0..data.len()).collect::<Vec<_>>();
///     indices.sort_by_key(|&i| &data[i]);
///     indices
/// };
/// 
/// sort_by_indices(|x, y| data.swap(x, y), &mut indices);
/// ```
/// While not very useful for single vectors, this can be useful for sorting sets of parallel
/// vectors where the element at `i` each vector needs to occupy the same index in all vectors.
/// 
/// # Arguments
/// * `len`     - The length of the sequence being sorted.
/// * `indices` - A pre-sorted set of indices where each element represents the current position of the element.
/// * `swap`    - A swap function called each time a pair of elements in the sequence is moved.
pub fn sort_by_indices<F>(len: usize, indices: &mut Vec<usize>, mut swap: F)
    where F: FnMut(usize, usize)
{
    for idx in 0..len {
        if indices[idx] != idx {
            let mut current_idx = idx;
            loop { 
                let target_idx          = indices[current_idx];
                indices[current_idx]    = current_idx;

                if indices[target_idx] == target_idx {
                    break;
                }

                swap(current_idx, target_idx);

                current_idx = target_idx;
            }
        }
    }
}

/// Parse a hex value to an HSL tuple.
fn hex_to_hsl(value: &str) -> Result<(f64, f64, f64), Error> {
    let (mut r, mut g, mut b) = hex_to_rgb(value)?;

    r          /= 255.0;
    g          /= 255.0;
    b          /= 255.0;

    let cmin    = r.min(g).min(b);
    let cmax    = r.max(g).max(b);
    let delta   = cmax - cmin;

    let mut h;
    let mut s;
    let mut l;

    if      delta == 0.0 { h = 0.0; }
    else if cmax  == r   { h = ((g - b) / delta).rem_euclid(6.0); }
    else if cmax  == g   { h = (b - r) / delta + 2.0;   }
    else                 { h = (r - g) / delta + 4.0;   }

    h = (h * 60.0).round();

    if h < 0.0 { h += 360.0; }

    l = (cmax + cmin) / 2.0;
    s = if delta == 0.0 { 0.0 } else { delta / (1.0 - (2.0 * l - 1.0).abs()) };
    s = s * 100.0;
    l = l * 100.0;

    Ok((h, s, l))
}

/// Parse a hex value to an RGB tuple.
fn hex_to_rgb(value: &str) -> Result<(f64, f64, f64), Error> {
    // We expect either #RRGGBB format only.
    if value.len() != 7 {
        return Err(Box::new(ColorParseError::InvalidFormat));
    }

    // Make sure the leading hashtag is present.
    if !value.starts_with("#") {
        return Err(Box::new(ColorParseError::InvalidFormat));
    }

    // Parse the hex value.
    let hex = u32::from_str_radix(&value[1..], 16)?;

    let b = hex >> 0  & 0xff;
    let g = hex >> 8  & 0xff;
    let r = hex >> 16 & 0xff;

    Ok((r as f64, g as f64, b as f64))
}

/// Convert HSL to hex.
fn hsl_to_hex(h: f64, s: f64, l: f64) -> String {
    let (r, g, b) = hsl_to_rgb(h, s, l);
    let value     = (r << 16) | (g << 8) | (b << 0);

    format!("{:06x}", value)
}

/// Convert HSL to RGB.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u32, u32, u32) {
    let s = s.clamp(0.0, 100.0) / 100.0;
    let l = l.clamp(0.0, 100.0) / 100.0;

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (((h / 60.0).rem_euclid(2.0)) - 1.0).abs());
    let m = l - c / 2.0;

    let mut r = 0.0;
    let mut g = 0.0;
    let mut b = 0.0;

    if h >= 0.0 && h < 60.0 {
        r = c;
        g = x;
        b = 0.0;
    }
    else if h >= 60.0 && h < 120.0 {
        r = x;
        g = c;
        b = 0.0;
    }
    else if h >= 120.0 && h < 180.0 {
        r = 0.0;
        g = c;
        b = x;
    }
    else if h >= 180.0 && h < 240.0 {
        r = 0.0;
        g = x;
        b = c;
    }
    else if h >= 240.0 && h < 300.0 {
        r = x;
        g = 0.0;
        b = c;
    }
    else if h >= 300.0 && h < 360.0 {
        r = c;
        g = 0.0;
        b = x;
    }

    (
        ((r + m) * 255.0).round() as u32,
        ((g + m) * 255.0).round() as u32,
        ((b + m) * 255.0).round() as u32,
    )
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