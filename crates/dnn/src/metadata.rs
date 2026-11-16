//! A .NET assembly's identity and references read from its ECMA-335 metadata - what the C# reads with
//! `AssemblyName.GetAssemblyName` (`DnnPackageInstaller.StrongName`) and `System.Reflection.Metadata`
//! (`DnnSiteInspector.Assemblies`). The port has no .NET runtime: this reads the PE file's CLI header, the metadata
//! streams and the Assembly and AssemblyRef tables - nothing is loaded or run.

use std::path::Path;

use sha1::{Digest, Sha1};

use crate::version::Version;

/// The largest DLL read: DNN's biggest assemblies are a few MB.
const MAX_FILE: u64 = 256 * 1024 * 1024;

/// What an assembly says of itself and what it references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyInfo {
    pub name: String,
    pub version: Version,
    /// The public key token, 16 lowercase hex digits; None when it isn't strong-named.
    pub public_key_token: Option<String>,
    /// The assemblies it references, with the versions it was built against.
    pub references: Vec<(String, Version)>,
}

/// Reads `file`; None when it isn't a .NET assembly (a native DLL) or can't be read.
pub fn read(file: &Path) -> Option<AssemblyInfo> {
    if std::fs::metadata(file).ok()?.len() > MAX_FILE {
        return None;
    }
    parse(&std::fs::read(file).ok()?)
}

/// The strong name of an assembly: its name, public key token and version - None when it isn't .NET or not
/// strong-named (`DnnPackageInstaller.StrongName`).
pub fn strong_name(file: &Path) -> Option<(String, String, Version)> {
    let info = read(file)?;
    Some((info.name, info.public_key_token?, info.version))
}

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.0.get(at..at + 2)?.try_into().ok()?))
    }
    fn u32(&self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.0.get(at..at + 4)?.try_into().ok()?))
    }
    fn u64(&self, at: usize) -> Option<u64> {
        Some(u64::from_le_bytes(self.0.get(at..at + 8)?.try_into().ok()?))
    }
    /// A 2- or 4-byte index.
    fn index(&self, at: usize, wide: bool) -> Option<u32> {
        if wide { self.u32(at) } else { self.u16(at).map(u32::from) }
    }
}

/// The metadata tables up to AssemblyRef (0x23); later tables only count for the coded indexes.
const ASSEMBLY: usize = 0x20;
const ASSEMBLY_REF: usize = 0x23;

pub(crate) fn parse(data: &[u8]) -> Option<AssemblyInfo> {
    let b = Bytes(data);
    if data.get(0..2)? != b"MZ" {
        return None;
    }
    let pe = b.u32(0x3c)? as usize;
    if data.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let coff = pe + 4;
    let sections = b.u16(coff + 2)? as usize;
    let optional_size = b.u16(coff + 16)? as usize;
    let optional = coff + 20;
    let (directories, count_at) = match b.u16(optional)? {
        0x10b => (optional + 96, optional + 92),
        0x20b => (optional + 112, optional + 108),
        _ => return None,
    };
    if b.u32(count_at)? < 15 {
        return None;
    }
    let cli_rva = b.u32(directories + 14 * 8)?;
    if cli_rva == 0 {
        return None;
    }
    let section_table = optional + optional_size;
    let to_offset = |rva: u32| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = section_table + i * 40;
            let size = b.u32(s + 8)?.max(b.u32(s + 16)?);
            let address = b.u32(s + 12)?;
            (rva >= address && rva < address + size).then(|| Some((rva - address + b.u32(s + 20)?) as usize))?
        })
    };
    let cli = to_offset(cli_rva)?;
    let root = to_offset(b.u32(cli + 8)?)?;
    if b.u32(root)? != 0x424A_5342 {
        return None;
    }
    let version_length = b.u32(root + 12)? as usize;
    let mut at = root + 16 + version_length;
    let streams = b.u16(at + 2)? as usize;
    at += 4;
    let (mut tables, mut strings, mut blob) = (None, None, None);
    for _ in 0..streams {
        let offset = b.u32(at)? as usize;
        let size = b.u32(at + 4)? as usize;
        let name_start = at + 8;
        let name_end = name_start + data.get(name_start..)?.iter().position(|c| *c == 0)?;
        let stream = (root + offset, size);
        match data.get(name_start..name_end)? {
            b"#~" | b"#-" => tables = Some(stream),
            b"#Strings" => strings = Some(stream),
            b"#Blob" => blob = Some(stream),
            _ => {}
        }
        at = name_start + (name_end - name_start + 4) / 4 * 4;
    }
    let (tables, _) = tables?;
    let (strings, strings_size) = strings?;
    let blob = blob.map(|(start, _)| start);

    let heaps = data.get(tables + 6)?;
    let wide_string = heaps & 1 != 0;
    let wide_guid = heaps & 2 != 0;
    let wide_blob = heaps & 4 != 0;
    let valid = b.u64(tables + 8)?;
    let mut rows = [0u32; 64];
    let mut at = tables + 24;
    for (table, count) in rows.iter_mut().enumerate() {
        if valid & (1 << table) != 0 {
            *count = b.u32(at)?;
            at += 4;
        }
    }
    // An uncompressed (#-) stream with extra data has 4 more bytes here.
    if heaps & 0x40 != 0 {
        at += 4;
    }
    let s = if wide_string { 4 } else { 2 };
    let g = if wide_guid { 4 } else { 2 };
    let bl = if wide_blob { 4 } else { 2 };
    let idx = |table: usize| if rows[table] >= 0x1_0000 { 4 } else { 2 };
    let coded = |tables: &[usize], bits: u32| {
        let max = tables.iter().map(|t| rows[*t]).max().unwrap_or(0);
        if max >= 1 << (16 - bits) { 4 } else { 2 }
    };
    let type_def_or_ref = coded(&[0x02, 0x01, 0x1b], 2);
    let has_constant = coded(&[0x04, 0x08, 0x17], 2);
    let has_custom_attribute = coded(
        &[
            0x06, 0x04, 0x01, 0x02, 0x08, 0x09, 0x0a, 0x00, 0x0e, 0x17, 0x14, 0x11, 0x1a, 0x1b, 0x20, 0x23, 0x26, 0x27,
            0x28, 0x2a, 0x2c, 0x2b,
        ],
        5,
    );
    let custom_attribute_type = coded(&[0x06, 0x0a], 3);
    let has_field_marshal = coded(&[0x04, 0x08], 1);
    let has_decl_security = coded(&[0x02, 0x06, 0x20], 2);
    let member_ref_parent = coded(&[0x02, 0x01, 0x1a, 0x06, 0x1b], 3);
    let has_semantics = coded(&[0x14, 0x17], 1);
    let method_def_or_ref = coded(&[0x06, 0x0a], 1);
    let member_forwarded = coded(&[0x04, 0x06], 1);
    let resolution_scope = coded(&[0x00, 0x1a, 0x23, 0x01], 2);
    let row_size: [usize; ASSEMBLY_REF + 1] = [
        2 + s + 3 * g,                                       // Module
        resolution_scope + 2 * s,                            // TypeRef
        4 + 2 * s + type_def_or_ref + idx(0x04) + idx(0x06), // TypeDef
        idx(0x04),                                           // FieldPtr
        2 + s + bl,                                          // Field
        idx(0x06),                                           // MethodPtr
        8 + s + bl + idx(0x08),                              // MethodDef
        idx(0x08),                                           // ParamPtr
        4 + s,                                               // Param
        idx(0x02) + type_def_or_ref,                         // InterfaceImpl
        member_ref_parent + s + bl,                          // MemberRef
        2 + has_constant + bl,                               // Constant
        has_custom_attribute + custom_attribute_type + bl,   // CustomAttribute
        has_field_marshal + bl,                              // FieldMarshal
        2 + has_decl_security + bl,                          // DeclSecurity
        6 + idx(0x02),                                       // ClassLayout
        4 + idx(0x04),                                       // FieldLayout
        bl,                                                  // StandAloneSig
        idx(0x02) + idx(0x14),                               // EventMap
        idx(0x14),                                           // EventPtr
        2 + s + type_def_or_ref,                             // Event
        idx(0x02) + idx(0x17),                               // PropertyMap
        idx(0x17),                                           // PropertyPtr
        2 + s + bl,                                          // Property
        2 + idx(0x06) + has_semantics,                       // MethodSemantics
        idx(0x02) + 2 * method_def_or_ref,                   // MethodImpl
        s,                                                   // ModuleRef
        bl,                                                  // TypeSpec
        2 + member_forwarded + s + idx(0x1a),                // ImplMap
        4 + idx(0x04),                                       // FieldRVA
        8,                                                   // EncLog
        4,                                                   // EncMap
        16 + bl + 2 * s,                                     // Assembly
        4,                                                   // AssemblyProcessor
        12,                                                  // AssemblyOS
        12 + 2 * bl + 2 * s,                                 // AssemblyRef
    ];
    let mut start = [0usize; ASSEMBLY_REF + 1];
    for table in 0..=ASSEMBLY_REF {
        start[table] = at;
        at += row_size[table] * rows[table] as usize;
    }
    if rows[ASSEMBLY] == 0 {
        return None;
    }

    let string = |index: u32| -> Option<String> {
        let from = strings + index as usize;
        if index as usize >= strings_size {
            return None;
        }
        let end = from + data.get(from..)?.iter().position(|c| *c == 0)?;
        Some(String::from_utf8_lossy(data.get(from..end)?).into_owned())
    };
    let blob_bytes = |index: u32| -> Option<&[u8]> {
        let from = blob? + index as usize;
        let first = *data.get(from)?;
        let (length, header) = if first & 0x80 == 0 {
            (first as usize, 1)
        } else if first & 0xc0 == 0x80 {
            ((((first & 0x3f) as usize) << 8) | *data.get(from + 1)? as usize, 2)
        } else {
            let bytes = data.get(from..from + 4)?;
            (
                (((bytes[0] & 0x1f) as usize) << 24)
                    | (bytes[1] as usize) << 16
                    | (bytes[2] as usize) << 8
                    | bytes[3] as usize,
                4,
            )
        };
        data.get(from + header..from + header + length)
    };
    let version = |at: usize| -> Option<Version> {
        Some(Version::new4(b.u16(at)? as u32, b.u16(at + 2)? as u32, b.u16(at + 4)? as u32, b.u16(at + 6)? as u32))
    };

    let row = start[ASSEMBLY];
    let assembly_version = version(row + 4)?;
    let public_key = b.index(row + 16, wide_blob)?;
    let name = string(b.index(row + 16 + bl, wide_string)?)?;
    let public_key_token = blob_bytes(public_key).filter(|k| !k.is_empty()).map(|key| {
        // The token: the last 8 bytes of the key's SHA-1, reversed.
        let hash = Sha1::digest(key);
        hash[hash.len() - 8..].iter().rev().map(|b| format!("{b:02x}")).collect::<String>()
    });

    let mut references = Vec::new();
    for i in 0..rows[ASSEMBLY_REF] as usize {
        let row = start[ASSEMBLY_REF] + i * row_size[ASSEMBLY_REF];
        let referenced = string(b.index(row + 12 + bl, wide_string)?)?;
        references.push((referenced, version(row)?));
    }
    Some(AssemblyInfo { name, version: assembly_version, public_key_token, references })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A .NET assembly every Windows has, strong-named by Microsoft.
    #[cfg(windows)]
    fn framework_assembly(name: &str) -> Option<std::path::PathBuf> {
        let windows = std::env::var_os("SystemRoot")?;
        let path = Path::new(&windows).join(r"Microsoft.NET\Framework64\v4.0.30319").join(name);
        path.exists().then_some(path)
    }

    #[cfg(windows)]
    #[test]
    fn reads_a_framework_assemblys_strong_name_and_references() {
        let Some(path) = framework_assembly("System.Web.dll") else { return };
        let info = read(&path).expect("System.Web is .NET");
        assert_eq!(info.name, "System.Web");
        assert_eq!(info.version, Version::new4(4, 0, 0, 0));
        assert_eq!(info.public_key_token.as_deref(), Some("b03f5f7f11d50a3a"));
        assert!(info.references.iter().any(|(n, v)| n == "mscorlib" && *v == Version::new4(4, 0, 0, 0)));
    }

    #[cfg(windows)]
    #[test]
    fn a_native_dll_isnt_an_assembly() {
        let windows = std::env::var_os("SystemRoot").unwrap();
        assert!(read(&Path::new(&windows).join(r"System32\kernel32.dll")).is_none());
    }

    #[test]
    fn what_isnt_a_pe_file_is_nothing() {
        assert!(parse(b"not a dll").is_none());
        assert!(parse(b"MZ").is_none());
    }
}
