//! Кодировки файлов.

use std::fmt;
use std::str::FromStr;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, IBM866, KOI8_R, KOI8_U, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1251};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enc {
    Utf8,
    Utf16Le,
    Utf16Be,
    Cp1251,
    Cp866,
    Koi8R,
}

impl Enc {
    pub const ALL: [Enc; 6] = [
        Enc::Utf8,
        Enc::Utf16Le,
        Enc::Utf16Be,
        Enc::Cp1251,
        Enc::Cp866,
        Enc::Koi8R,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
            Self::Cp1251 => "CP1251",
            Self::Cp866 => "CP866",
            Self::Koi8R => "KOI8-R",
        }
    }

    fn codec(self) -> &'static Encoding {
        match self {
            Self::Utf8 => UTF_8,
            Self::Utf16Le => UTF_16LE,
            Self::Utf16Be => UTF_16BE,
            Self::Cp1251 => WINDOWS_1251,
            Self::Cp866 => IBM866,
            Self::Koi8R => KOI8_R,
        }
    }

    fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf8 => b"\xef\xbb\xbf",
            Self::Utf16Le => b"\xff\xfe",
            Self::Utf16Be => b"\xfe\xff",
            _ => b"",
        }
    }
}

impl FromStr for Enc {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|e| e.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| {
                let names = Self::ALL.map(Self::name).join(", ");
                format!("unknown encoding, expected one of: {names}")
            })
    }
}

/// Кодировка файла и наличие BOM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub enc: Enc,
    pub bom: bool,
}

impl Format {
    pub const UTF8: Format = Format {
        enc: Enc::Utf8,
        bom: false,
    };

    /// Формат по умолчанию при выборе кодировки: UTF-16 пишется с BOM.
    pub fn of(enc: Enc) -> Self {
        Self {
            enc,
            bom: matches!(enc, Enc::Utf16Le | Enc::Utf16Be),
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.enc.name())?;
        if self.bom && self.enc == Enc::Utf8 {
            f.write_str(" BOM")?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Decoded {
    pub text: String,
    pub format: Format,
    /// Обратное кодирование не даёт исходных байтов: файл повреждён или
    /// кодировка не та.
    pub lossy: bool,
}

/// Символ, которого нет в кодировке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unencodable(pub char);

/// Декодирует файл. Без `forced` кодировка определяется по содержимому.
pub fn decode(bytes: &[u8], forced: Option<Enc>) -> Decoded {
    let enc = forced.unwrap_or_else(|| detect(bytes));
    let bom = !enc.bom().is_empty() && bytes.starts_with(enc.bom());
    let body = if bom {
        &bytes[enc.bom().len()..]
    } else {
        bytes
    };
    let (text, _) = enc.codec().decode_without_bom_handling(body);
    let text = text.into_owned();
    let format = Format { enc, bom };
    let lossy = encode(&text, format).as_deref() != Ok(bytes);
    Decoded {
        text,
        format,
        lossy,
    }
}

fn detect(bytes: &[u8]) -> Enc {
    for enc in [Enc::Utf8, Enc::Utf16Le, Enc::Utf16Be] {
        if bytes.starts_with(enc.bom()) {
            return enc;
        }
    }
    if let Some(enc) = detect_utf16(bytes) {
        return enc;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return Enc::Utf8;
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    // UTF-8 здесь уже исключён.
    match detector.guess(Some(b"ru"), Utf8Detection::Deny) {
        e if e == IBM866 => Enc::Cp866,
        e if e == KOI8_U || e == KOI8_R => Enc::Koi8R,
        _ => Enc::Cp1251,
    }
}

/// UTF-16 без BOM: у латиницы и кириллицы старший байт равен 0 или 4.
fn detect_utf16(bytes: &[u8]) -> Option<Enc> {
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let share = |first: usize| {
        let high = bytes.iter().skip(first).step_by(2);
        high.filter(|b| matches!(**b, 0 | 4)).count() * 2 * 10 / bytes.len()
    };
    match (share(1), share(0)) {
        (le, be) if le >= 9 && le > be => Some(Enc::Utf16Le),
        (le, be) if be >= 9 && be > le => Some(Enc::Utf16Be),
        _ => None,
    }
}

pub fn encode(text: &str, format: Format) -> Result<Vec<u8>, Unencodable> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if format.bom {
        out.extend_from_slice(format.enc.bom());
    }
    match format.enc {
        Enc::Utf8 => out.extend_from_slice(text.as_bytes()),
        Enc::Utf16Le => out.extend(text.encode_utf16().flat_map(u16::to_le_bytes)),
        Enc::Utf16Be => out.extend(text.encode_utf16().flat_map(u16::to_be_bytes)),
        _ => {
            let (bytes, _, unmappable) = format.enc.codec().encode(text);
            if unmappable {
                return Err(first_unencodable(text, format.enc));
            }
            out.extend_from_slice(&bytes);
        }
    }
    Ok(out)
}

fn first_unencodable(text: &str, enc: Enc) -> Unencodable {
    let bad = |c: &char| enc.codec().encode(c.encode_utf8(&mut [0; 4])).2;
    Unencodable(
        text.chars()
            .find(bad)
            .unwrap_or(char::REPLACEMENT_CHARACTER),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES: [&str; 5] = [
        "Привет",
        "# Заголовок\n\nОбычный текст заметки.\n",
        "ЗАГОЛОВОК",
        "- да\r\n- нет\r\n",
        "| имя | год |\n|---|---|\n| Анна | 1999 |\n",
    ];

    #[test]
    fn every_format_is_detected_and_round_trips() {
        for text in SAMPLES {
            for enc in Enc::ALL {
                for bom in [false, true] {
                    if bom && enc.bom().is_empty() {
                        continue;
                    }
                    let format = Format { enc, bom };
                    let bytes = encode(text, format).unwrap();
                    let d = decode(&bytes, None);
                    assert_eq!(d.format, format, "{text:?}");
                    assert_eq!(d.text, text);
                    assert!(!d.lossy);
                    assert_eq!(encode(&d.text, d.format).unwrap(), bytes);
                }
            }
        }
    }

    #[test]
    fn single_byte_encodings_preserve_every_byte() {
        let all: Vec<u8> = (0..=255).collect();
        for enc in [Enc::Cp1251, Enc::Cp866, Enc::Koi8R] {
            let d = decode(&all, Some(enc));
            assert!(!d.lossy, "{}", enc.name());
            assert_eq!(encode(&d.text, d.format).unwrap(), all);
        }
    }

    #[test]
    fn unencodable_character_is_reported() {
        let cp866 = Format::of(Enc::Cp866);
        assert_eq!(encode("тест — да", cp866), Err(Unencodable('—')));
        assert_eq!(
            encode("😀", Format::of(Enc::Cp1251)),
            Err(Unencodable('😀'))
        );
    }

    #[test]
    fn damage_is_noticed() {
        assert!(decode(b"abc\xff", Some(Enc::Utf8)).lossy);
        assert!(decode(b"\xff\xfea\0b", None).lossy, "odd length");
        assert!(decode(b"\xff\xfe\x00\xd8", None).lossy, "lone surrogate");
    }

    #[test]
    fn wrong_forced_encoding_still_keeps_bytes_when_possible() {
        let bytes = "Привет".as_bytes();
        let d = decode(bytes, Some(Enc::Cp1251));
        assert!(!d.lossy);
        assert_ne!(d.text, "Привет");
    }

    #[test]
    fn plain_ascii_and_empty_are_utf8() {
        assert_eq!(decode(b"", None).format, Format::UTF8);
        assert_eq!(decode(b"hello\n", None).format, Format::UTF8);
    }

    #[test]
    fn names() {
        assert_eq!("cp1251".parse(), Ok(Enc::Cp1251));
        assert_eq!("UTF-16le".parse(), Ok(Enc::Utf16Le));
        assert!("latin1".parse::<Enc>().is_err());
    }
}
