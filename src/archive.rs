use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use crate::resource_types::{extension_for, type_for};

const HEADER_SIZE: u64 = 160;
const MAX_ARCHIVE_ENTRIES: usize = 1_000_000;
const MAX_LOCALIZED_STRING_TABLE_SIZE: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveVersion {
    V1_0,
    V1_1,
}

impl ArchiveVersion {
    pub fn label(self) -> &'static str {
        match self {
            Self::V1_0 => "V1.0 (NWN/EE)",
            Self::V1_1 => "V1.1 (NWN2)",
        }
    }
    fn bytes(self) -> &'static [u8; 4] {
        match self {
            Self::V1_0 => b"V1.0",
            Self::V1_1 => b"V1.1",
        }
    }
    fn name_len(self) -> usize {
        match self {
            Self::V1_0 => 16,
            Self::V1_1 => 32,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveKind {
    Hak,
    Erf,
    Mod,
    Sav,
}

impl ArchiveKind {
    pub fn signature(self) -> &'static [u8; 4] {
        match self {
            Self::Hak => b"HAK ",
            Self::Erf => b"ERF ",
            Self::Mod | Self::Sav => b"MOD ",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Hak => "hak",
            Self::Erf => "erf",
            Self::Mod => "mod",
            Self::Sav => "sav",
        }
    }
}

#[derive(Clone, Debug)]
pub enum EntryData {
    ArchiveSlice {
        path: PathBuf,
        offset: u64,
        size: u64,
    },
    ExternalFile(PathBuf),
    #[allow(dead_code)]
    Memory(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub type_id: u16,
    pub data: EntryData,
}

impl Entry {
    pub fn extension(&self) -> String {
        extension_for(self.type_id)
    }
    pub fn is_new(&self) -> bool {
        matches!(self.data, EntryData::ExternalFile(_) | EntryData::Memory(_))
    }
    pub fn filename(&self) -> String {
        format!("{}.{}", self.name, self.extension())
    }
    pub fn safe_filename(&self) -> io::Result<String> {
        validate_archive_resource_name(&self.name, usize::MAX)?;
        Ok(self.filename())
    }
    pub fn size(&self) -> io::Result<u64> {
        match &self.data {
            EntryData::ArchiveSlice { size, .. } => Ok(*size),
            EntryData::ExternalFile(path) => Ok(fs::metadata(path)?.len()),
            EntryData::Memory(data) => Ok(data.len() as u64),
        }
    }
    fn copy_to(&self, output: &mut impl Write) -> io::Result<u64> {
        match &self.data {
            EntryData::ArchiveSlice { path, offset, size } => {
                let mut file = BufReader::new(File::open(path)?);
                file.seek(SeekFrom::Start(*offset))?;
                io::copy(&mut file.take(*size), output)
            }
            EntryData::ExternalFile(path) => {
                io::copy(&mut BufReader::new(File::open(path)?), output)
            }
            EntryData::Memory(data) => {
                output.write_all(data)?;
                Ok(data.len() as u64)
            }
        }
    }

    pub fn read_prefix(&self, limit: u64) -> io::Result<Vec<u8>> {
        let mut data = Vec::new();
        match &self.data {
            EntryData::ArchiveSlice { path, offset, size } => {
                let mut file = BufReader::new(File::open(path)?);
                file.seek(SeekFrom::Start(*offset))?;
                file.take((*size).min(limit)).read_to_end(&mut data)?;
            }
            EntryData::ExternalFile(path) => {
                BufReader::new(File::open(path)?)
                    .take(limit)
                    .read_to_end(&mut data)?;
            }
            EntryData::Memory(bytes) => {
                data.extend_from_slice(&bytes[..bytes.len().min(limit as usize)])
            }
        }
        Ok(data)
    }
}

#[derive(Clone, Debug)]
struct LocalizedString {
    language_id: u32,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Archive {
    pub kind: ArchiveKind,
    pub version: ArchiveVersion,
    pub path: Option<PathBuf>,
    pub entries: Vec<Entry>,
    localized: Vec<LocalizedString>,
    description_strref: u32,
}

impl Archive {
    pub fn new(kind: ArchiveKind, version: ArchiveVersion) -> Self {
        let description = if kind == ArchiveKind::Hak {
            b"Aurora Hak Explorer\nCreated with AHE".to_vec()
        } else {
            Vec::new()
        };
        Self {
            kind,
            version,
            path: None,
            entries: Vec::new(),
            localized: vec![LocalizedString {
                language_id: 0,
                bytes: description,
            }],
            description_strref: u32::MAX,
        }
    }

    pub fn description(&self) -> String {
        self.localized
            .first()
            .map(|s| String::from_utf8_lossy(&s.bytes).into_owned())
            .unwrap_or_default()
    }

    pub fn set_description(&mut self, text: String) {
        if let Some(first) = self.localized.first_mut() {
            first.bytes = text.into_bytes();
        } else {
            self.localized.push(LocalizedString {
                language_id: 0,
                bytes: text.into_bytes(),
            });
        }
    }

    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file_len = fs::metadata(&path)?.len();
        if file_len < HEADER_SIZE {
            return Err(invalid("file is too small to be an ERF archive"));
        }
        let mut input = BufReader::new(File::open(&path)?);
        let mut signature = [0; 4];
        input.read_exact(&mut signature)?;
        let kind = match &signature {
            b"HAK " => ArchiveKind::Hak,
            b"ERF " => ArchiveKind::Erf,
            b"MOD " => {
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("sav"))
                {
                    ArchiveKind::Sav
                } else {
                    ArchiveKind::Mod
                }
            }
            _ => return Err(invalid("unsupported archive signature")),
        };
        let mut version_bytes = [0; 4];
        input.read_exact(&mut version_bytes)?;
        let version = match &version_bytes {
            b"V1.0" => ArchiveVersion::V1_0,
            b"V1.1" => ArchiveVersion::V1_1,
            _ => return Err(invalid("unsupported ERF version")),
        };
        let language_count = read_u32(&mut input)? as usize;
        let localized_size = read_u32(&mut input)? as u64;
        let entry_count = read_u32(&mut input)? as usize;
        if localized_size > MAX_LOCALIZED_STRING_TABLE_SIZE {
            return Err(invalid("localized string table is too large"));
        }
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(invalid(format!(
                "archive contains more than {MAX_ARCHIVE_ENTRIES} resources"
            )));
        }
        let localized_offset = read_u32(&mut input)? as u64;
        let key_offset = read_u32(&mut input)? as u64;
        let resource_offset = read_u32(&mut input)? as u64;
        let _build_year = read_u32(&mut input)?;
        let _build_day = read_u32(&mut input)?;
        let description_strref = read_u32(&mut input)?;

        let name_len = version.name_len();
        check_range(
            localized_offset,
            localized_size,
            file_len,
            "localized strings",
        )?;
        let minimum_localized_size = (language_count as u64)
            .checked_mul(8)
            .ok_or_else(|| invalid("localized string table is too large"))?;
        if minimum_localized_size > localized_size {
            return Err(invalid(
                "localized string count exceeds the localized string table",
            ));
        }
        check_range(
            key_offset,
            (entry_count as u64) * (name_len as u64 + 8),
            file_len,
            "key list",
        )?;
        check_range(
            resource_offset,
            (entry_count as u64) * 8,
            file_len,
            "resource list",
        )?;

        input.seek(SeekFrom::Start(localized_offset))?;
        let mut localized = Vec::new();
        localized
            .try_reserve(language_count)
            .map_err(|_| invalid("localized string table is too large"))?;
        let localized_end = localized_offset + localized_size;
        for index in 0..language_count {
            let language_id = read_u32(&mut input)?;
            let declared_size = read_u32(&mut input)? as u64;
            let string_start = input.stream_position()?;
            let remaining = localized_end.saturating_sub(string_start);
            // Some archives produced by Gareth Hughes' old command-line `erf`
            // utility store the size of the entire localized-string table in
            // the sole string's length field. In that specific layout the
            // declared length is eight bytes too large because it includes the
            // language-id and length fields themselves. Accept only this exact
            // legacy shape; other out-of-range strings remain errors.
            let size = if declared_size > remaining
                && language_count == 1
                && index == 0
                && declared_size == localized_size
                && string_start == localized_offset + 8
            {
                remaining
            } else if declared_size > remaining {
                return Err(invalid("localized string exceeds its table"));
            } else {
                declared_size
            };
            let size =
                usize::try_from(size).map_err(|_| invalid("localized string is too large"))?;
            let mut bytes = vec![0; size];
            input.read_exact(&mut bytes)?;
            localized.push(LocalizedString { language_id, bytes });
        }

        #[derive(Debug)]
        struct Key {
            name: String,
            type_id: u16,
        }
        input.seek(SeekFrom::Start(key_offset))?;
        let mut keys = Vec::new();
        keys.try_reserve(entry_count)
            .map_err(|_| invalid("archive key list is too large"))?;
        for _ in 0..entry_count {
            let mut name_bytes = vec![0; name_len];
            input.read_exact(&mut name_bytes)?;
            let end = name_bytes.iter().position(|b| *b == 0).unwrap_or(name_len);
            let name = String::from_utf8_lossy(&name_bytes[..end])
                .trim_end()
                .to_owned();
            let _resource_id = read_u32(&mut input)?;
            let type_id = read_u16(&mut input)?;
            let _unused = read_u16(&mut input)?;
            if name.is_empty() {
                return Err(invalid("archive contains an empty resource name"));
            }
            validate_archive_resource_name(&name, name_len)?;
            keys.push(Key { name, type_id });
        }

        input.seek(SeekFrom::Start(resource_offset))?;
        let mut entries = Vec::new();
        entries
            .try_reserve(entry_count)
            .map_err(|_| invalid("archive resource list is too large"))?;
        for key in keys {
            let offset = read_u32(&mut input)? as u64;
            let size = read_u32(&mut input)? as u64;
            check_range(offset, size, file_len, "resource data")?;
            entries.push(Entry {
                name: key.name,
                type_id: key.type_id,
                data: EntryData::ArchiveSlice {
                    path: path.clone(),
                    offset,
                    size,
                },
            });
        }
        entries.sort_by_key(|e| (e.name.to_ascii_lowercase(), e.type_id));
        Ok(Self {
            kind,
            version,
            path: Some(path),
            entries,
            localized,
            description_strref,
        })
    }

    pub fn add_file(&mut self, path: impl AsRef<Path>) -> io::Result<bool> {
        let entry = self.entry_from_file(path.as_ref())?;
        let replacement = self.entries.iter().position(|existing| {
            existing.name.eq_ignore_ascii_case(&entry.name) && existing.type_id == entry.type_id
        });
        if let Some(index) = replacement {
            self.entries[index] = entry;
        } else {
            self.entries.push(entry);
        }
        self.entries
            .sort_by_key(|entry| (entry.name.to_ascii_lowercase(), entry.type_id));
        Ok(replacement.is_some())
    }

    pub fn conflicting_filename(&self, path: impl AsRef<Path>) -> io::Result<Option<String>> {
        let incoming = self.entry_from_file(path.as_ref())?;
        Ok(self
            .entries
            .iter()
            .find(|existing| {
                existing.name.eq_ignore_ascii_case(&incoming.name)
                    && existing.type_id == incoming.type_id
            })
            .map(Entry::filename))
    }

    fn entry_from_file(&self, path: &Path) -> io::Result<Entry> {
        if !path.is_file() {
            return Err(invalid("resource is not a regular file"));
        }
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| invalid("resource filename is not valid UTF-8"))?;
        let (raw_name, extension) = file_name
            .rsplit_once('.')
            .ok_or_else(|| invalid("resource filename has no extension"))?;
        let type_id = type_for(extension)
            .ok_or_else(|| invalid(format!("unknown NWN resource extension: .{extension}")))?;
        let max = self.version.name_len();
        let name = sanitize_name(raw_name, max)?;
        Ok(Entry {
            name,
            type_id,
            data: EntryData::ExternalFile(path.to_path_buf()),
        })
    }

    pub fn merge(&mut self, other: &Archive) -> (usize, usize) {
        let mut added = 0;
        let mut replaced = 0;
        for incoming in &other.entries {
            if let Some(index) = self.entries.iter().position(|e| {
                e.name.eq_ignore_ascii_case(&incoming.name) && e.type_id == incoming.type_id
            }) {
                self.entries[index] = incoming.clone();
                replaced += 1;
            } else {
                self.entries.push(incoming.clone());
                added += 1;
            }
        }
        self.entries
            .sort_by_key(|e| (e.name.to_ascii_lowercase(), e.type_id));
        (added, replaced)
    }

    pub fn export_entry(&self, index: usize, output: impl AsRef<Path>) -> io::Result<()> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| invalid("resource index is out of range"))?;
        let mut writer = BufWriter::new(File::create(output)?);
        entry.copy_to(&mut writer)?;
        writer.flush()
    }

    pub fn extract_all(&self, directory: impl AsRef<Path>) -> io::Result<usize> {
        fs::create_dir_all(&directory)?;
        for (index, entry) in self.entries.iter().enumerate() {
            self.export_entry(index, directory.as_ref().join(entry.safe_filename()?))?;
        }
        Ok(self.entries.len())
    }

    pub fn save(&mut self, output: impl AsRef<Path>) -> io::Result<()> {
        let output = output.as_ref();
        let existing_permissions = fs::metadata(output)
            .ok()
            .map(|metadata| metadata.permissions());
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        self.write_to(temp.as_file_mut())?;
        temp.as_file_mut().sync_all()?;
        temp.persist(output).map_err(|e| e.error)?;
        if let Some(permissions) = existing_permissions {
            fs::set_permissions(output, permissions)?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(output, fs::Permissions::from_mode(0o644))?;
            }
        }
        *self = Self::open(output)?;
        Ok(())
    }

    fn write_to(&self, file: &mut File) -> io::Result<()> {
        let count = self.entries.len();
        if count > u32::MAX as usize {
            return Err(invalid("too many resources"));
        }
        let localized_size: u64 = self
            .localized
            .iter()
            .map(|s| 8 + s.bytes.len() as u64)
            .sum();
        let key_offset = HEADER_SIZE
            .checked_add(localized_size)
            .ok_or_else(|| invalid("archive is too large"))?;
        let key_size = (count as u64)
            .checked_mul((self.version.name_len() + 8) as u64)
            .ok_or_else(|| invalid("archive is too large"))?;
        let resource_offset = key_offset
            .checked_add(key_size)
            .ok_or_else(|| invalid("archive is too large"))?;
        let data_offset = resource_offset
            .checked_add((count as u64) * 8)
            .ok_or_else(|| invalid("archive is too large"))?;
        for value in [localized_size, key_offset, resource_offset, data_offset] {
            u32::try_from(value).map_err(|_| invalid("ERF V1.x offsets cannot exceed 4 GiB"))?;
        }

        let mut output = BufWriter::new(file);
        output.write_all(self.kind.signature())?;
        output.write_all(self.version.bytes())?;
        write_u32(&mut output, self.localized.len() as u32)?;
        write_u32(&mut output, localized_size as u32)?;
        write_u32(&mut output, count as u32)?;
        write_u32(&mut output, HEADER_SIZE as u32)?;
        write_u32(&mut output, key_offset as u32)?;
        write_u32(&mut output, resource_offset as u32)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let (build_year, build_day) = erf_build_date(now / 86_400);
        write_u32(&mut output, build_year)?;
        write_u32(&mut output, build_day)?;
        write_u32(&mut output, self.description_strref)?;
        output.write_all(&[0; 116])?;
        for string in &self.localized {
            write_u32(&mut output, string.language_id)?;
            write_u32(&mut output, string.bytes.len() as u32)?;
            output.write_all(&string.bytes)?;
        }
        for (id, entry) in self.entries.iter().enumerate() {
            let bytes = entry.name.as_bytes();
            let max = self.version.name_len();
            if bytes.len() > max {
                return Err(invalid(format!(
                    "resource name '{}' exceeds {max} bytes",
                    entry.name
                )));
            }
            output.write_all(bytes)?;
            output.write_all(&vec![0; max - bytes.len()])?;
            write_u32(&mut output, id as u32)?;
            write_u16(&mut output, entry.type_id)?;
            write_u16(&mut output, 0)?;
        }
        let mut offset = data_offset;
        for entry in &self.entries {
            let size = entry.size()?;
            let offset32 =
                u32::try_from(offset).map_err(|_| invalid("ERF V1.x archive exceeds 4 GiB"))?;
            let size32 = u32::try_from(size).map_err(|_| invalid("a resource exceeds 4 GiB"))?;
            write_u32(&mut output, offset32)?;
            write_u32(&mut output, size32)?;
            offset = offset
                .checked_add(size)
                .ok_or_else(|| invalid("archive is too large"))?;
        }
        for entry in &self.entries {
            entry.copy_to(&mut output)?;
        }
        output.flush()
    }
}

fn erf_build_date(mut days_since_epoch: u64) -> (u32, u32) {
    let mut year = 1970_u32;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days_since_epoch < days_in_year {
            return (year - 1900, days_since_epoch as u32 + 1);
        }
        days_since_epoch -= days_in_year;
        year += 1;
    }
}

fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn sanitize_name(raw: &str, max: usize) -> io::Result<String> {
    let lowered = raw.to_ascii_lowercase();
    if lowered.is_empty()
        || lowered.len() > max
        || !lowered
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(invalid(format!(
            "resource name must be 1-{max} ASCII letters, digits, or underscores"
        )));
    }
    Ok(lowered)
}

fn validate_archive_resource_name(name: &str, max: usize) -> io::Result<()> {
    if name.is_empty()
        || name.len() > max
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
    {
        return Err(invalid(
            "archive contains an unsafe resource name with path or control characters",
        ));
    }
    Ok(())
}
fn check_range(offset: u64, size: u64, file_len: u64, label: &str) -> io::Result<()> {
    if offset.checked_add(size).is_none_or(|end| end > file_len) {
        Err(invalid(format!("{label} lies outside the file")))
    } else {
        Ok(())
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn read_u16(input: &mut impl Read) -> io::Result<u16> {
    let mut b = [0; 2];
    input.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut b = [0; 4];
    input.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn write_u16(output: &mut impl Write, n: u16) -> io::Result<()> {
    output.write_all(&n.to_le_bytes())
}
fn write_u32(output: &mut impl Write, n: u32) -> io::Result<()> {
    output.write_all(&n.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_v10() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("test.hak");
        let mut archive = Archive::new(ArchiveKind::Hak, ArchiveVersion::V1_0);
        archive.entries.push(Entry {
            name: "sample".into(),
            type_id: 0x07e1,
            data: EntryData::Memory(b"2DA V2.0\n".to_vec()),
        });
        archive.save(&output).unwrap();
        let loaded = Archive::open(&output).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].filename(), "sample.2da");
        let extracted = dir.path().join("sample.2da");
        loaded.export_entry(0, &extracted).unwrap();
        assert_eq!(fs::read(extracted).unwrap(), b"2DA V2.0\n");
    }
    #[test]
    fn rejects_bad_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.hak");
        fs::write(&path, b"HAK V1.0").unwrap();
        assert!(Archive::open(path).is_err());
    }

    #[test]
    fn accepts_legacy_erf_tool_localized_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.hak");
        let mut archive = Archive::new(ArchiveKind::Hak, ArchiveVersion::V1_0);
        archive.set_description("Created by the old erf utility".into());
        archive.save(&path).unwrap();

        // Reproduce the old utility's bug: the string length includes its own
        // eight-byte localized-table record header.
        let mut bytes = fs::read(&path).unwrap();
        let localized_size = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        bytes[164..168].copy_from_slice(&localized_size.to_le_bytes());
        fs::write(&path, bytes).unwrap();

        let loaded = Archive::open(&path).unwrap();
        assert_eq!(loaded.description(), "Created by the old erf utility");
    }

    #[test]
    fn rejects_localized_count_larger_than_its_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad-count.hak");
        let mut archive = Archive::new(ArchiveKind::Hak, ArchiveVersion::V1_0);
        archive.save(&path).unwrap();

        let mut bytes = fs::read(&path).unwrap();
        bytes[8..12].copy_from_slice(&1_000_000_u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();

        let error = Archive::open(path).unwrap_err();
        assert!(error.to_string().contains("count exceeds"));
    }

    #[test]
    fn rejects_unsafe_resource_names_before_they_can_escape_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unsafe-name.hak");
        let mut archive = Archive::new(ArchiveKind::Hak, ArchiveVersion::V1_0);
        archive.entries.push(Entry {
            name: "sample".into(),
            type_id: 0x000a,
            data: EntryData::Memory(b"test".to_vec()),
        });
        archive.save(&path).unwrap();

        let mut bytes = fs::read(&path).unwrap();
        let key_offset = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
        bytes[key_offset..key_offset + 16].fill(0);
        bytes[key_offset..key_offset + 9].copy_from_slice(b"../escape");
        fs::write(&path, bytes).unwrap();

        let error = Archive::open(path).unwrap_err();
        assert!(error.to_string().contains("unsafe resource name"));
    }

    #[test]
    fn extraction_rechecks_names_and_preserves_real_world_compatibility() {
        for name in ["Metal weathered", "cav_copper-01", "cat_range_+1"] {
            validate_archive_resource_name(name, 16).unwrap();
        }
        for name in ["../escape", r"C:\escape", "bad:name", "line\nbreak"] {
            assert!(validate_archive_resource_name(name, 16).is_err());
        }

        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("output");
        let mut archive = Archive::new(ArchiveKind::Hak, ArchiveVersion::V1_0);
        archive.entries.push(Entry {
            name: "../escape".into(),
            type_id: 0x000a,
            data: EntryData::Memory(b"must not escape".to_vec()),
        });
        assert!(archive.extract_all(&output).is_err());
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn rejects_impractical_resource_counts_before_allocating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("too-many.hak");
        let mut archive = Archive::new(ArchiveKind::Hak, ArchiveVersion::V1_0);
        archive.save(&path).unwrap();

        let mut bytes = fs::read(&path).unwrap();
        bytes[16..20].copy_from_slice(&((MAX_ARCHIVE_ENTRIES as u32) + 1).to_le_bytes());
        fs::write(&path, bytes).unwrap();

        let error = Archive::open(path).unwrap_err();
        assert!(error.to_string().contains("more than"));
    }

    #[test]
    fn erf_build_dates_observe_leap_years() {
        assert_eq!(erf_build_date(0), (70, 1));
        assert_eq!(erf_build_date(365 + 365), (72, 1));
        assert_eq!(erf_build_date(365 + 365 + 365), (72, 366));
        assert_eq!(erf_build_date(365 + 365 + 366), (73, 1));
    }

    #[test]
    fn adds_common_and_enhanced_edition_resources() {
        let dir = tempfile::tempdir().unwrap();
        let mut archive = Archive::new(ArchiveKind::Hak, ArchiveVersion::V1_0);
        for (name, expected_type) in [
            ("model.mdl", 0x07d2),
            ("texture.dds", 0x07f1),
            ("table.2da", 0x07e1),
            ("material.mtr", 0x0818),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, b"test").unwrap();
            archive.add_file(path).unwrap();
            assert!(
                archive
                    .entries
                    .iter()
                    .any(|entry| entry.type_id == expected_type)
            );
        }
        assert_eq!(
            archive
                .conflicting_filename(dir.path().join("model.mdl"))
                .unwrap()
                .as_deref(),
            Some("model.mdl")
        );
        let different_type = dir.path().join("model.dds");
        fs::write(&different_type, b"test").unwrap();
        assert_eq!(archive.conflicting_filename(different_type).unwrap(), None);
    }
}
