//! Adobe Color URL importer — query-string / form decoding only.

use super::types::{Palette, Swatch, DEFAULT_SLOTS};

/// Decode `application/x-www-form-urlencoded` (`+` and `%xx`).
pub fn form_decode(input: &str) -> String {
    let plus = input.replace('+', " ");
    let bytes = plus.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(url: &str) -> Result<Vec<(String, String)>, String> {
    let q = url
        .split_once('?')
        .map(|(_, rest)| rest.split('#').next().unwrap_or(rest))
        .unwrap_or(url);
    if q.is_empty() {
        return Err("Adobe Color URL has no query string".into());
    }
    let mut pairs = Vec::new();
    for part in q.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        pairs.push((form_decode(k), form_decode(v)));
    }
    Ok(pairs)
}

/// Parse `https://color.adobe.com/...` (or a raw query) into a palette.
pub fn import_adobe_url(url: &str, id: &str) -> Result<Palette, String> {
    let pairs = parse_query(url)?;
    let name = pairs
        .iter()
        .find(|(k, _)| k == "name")
        .map(|(_, v)| v.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.to_string());

    let rgbvalues = pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("rgbvalues"))
        .map(|(_, v)| v.as_str())
        .ok_or_else(|| "Adobe Color URL missing rgbvalues".to_string())?;

    let nums: Vec<f32> = rgbvalues
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<f32>().map_err(|_| format!("bad rgbvalues number '{s}'")))
        .collect::<Result<_, _>>()?;

    if nums.len() < 3 || nums.len() % 3 != 0 {
        return Err(format!(
            "rgbvalues must be groups of 3 floats (got {})",
            nums.len()
        ));
    }

    let mut swatches = Vec::new();
    for (i, chunk) in nums.chunks(3).enumerate() {
        if i >= DEFAULT_SLOTS.len() {
            break;
        }
        swatches.push(Swatch {
            slot: DEFAULT_SLOTS[i].to_string(),
            rgba: [chunk[0], chunk[1], chunk[2], 1.0],
        });
    }
    while swatches.len() < DEFAULT_SLOTS.len() {
        let i = swatches.len();
        swatches.push(Swatch {
            slot: DEFAULT_SLOTS[i].to_string(),
            rgba: [0.0, 0.0, 0.0, 1.0],
        });
    }

    Ok(Palette {
        schema_version: 1,
        id: id.to_string(),
        name,
        swatches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_commas_and_plus() {
        let url = "https://color.adobe.com/foo?name=Deep+Earth&rgbvalues=0.3%2C0.22%2C0.18%2C0.2%2C0.15%2C0.12%2C0.1%2C0.1%2C0.1%2C0.08%2C0.07%2C0.06%2C0.05%2C0.04%2C0.03%2C0.12%2C0.1%2C0.09";
        let p = import_adobe_url(url, "earth").unwrap();
        assert_eq!(p.name, "Deep Earth");
        assert!((p.swatches[0].rgba[0] - 0.3).abs() < 1e-5);
        assert_eq!(p.swatches.len(), 6);
    }

    #[test]
    fn malformed_url_errors() {
        assert!(import_adobe_url("https://color.adobe.com/foo", "x").is_err());
        assert!(import_adobe_url("https://color.adobe.com/foo?rgbvalues=nope", "x").is_err());
        assert!(import_adobe_url("https://color.adobe.com/foo?rgbvalues=1,2", "x").is_err());
    }
}
