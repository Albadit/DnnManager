//! Just enough of a .NET assembly's metadata (ECMA-335, partition II) to read what `AssemblyInspector` reads with
//! `System.Reflection.Metadata`: its name, version and public key, and the assemblies it references. The file is
//! only read - never loaded or run - and anything that doesn't look like metadata is None: a native DLL.

use std::fmt;

/// An assembly version: major.minor.build.revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AssemblyVersion(pub u16, pub u16, pub u16, pub u16);

impl AssemblyVersion {
    /// "13.0.0.0", "13.0" (the parts left out are 0); None for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<u16> = text.trim().split('.').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        let at = |i: usize| parts.get(i).copied().unwrap_or(0);
        Some(Self(at(0), at(1), at(2), at(3)))
    }
}

impl fmt::Display for AssemblyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0, self.1, self.2, self.3)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyReference {
    pub name: String,
    pub version: AssemblyVersion,
    /// It names a public key (token): the runtime wants exactly this version, unless a redirect says otherwise.
    pub strong_named: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyMetadata {
    pub name: String,
    pub version: AssemblyVersion,
    pub public_key: Vec<u8>,
    pub references: Vec<AssemblyReference>,
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u8(&self, at: usize) -> Option<u8> {
        self.data.get(at).copied()
    }
    fn u16(&self, at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.data.get(at..at + 2)?.try_into().ok()?))
    }
    fn u32(&self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.data.get(at..at + 4)?.try_into().ok()?))
    }
    fn u64(&self, at: usize) -> Option<u64> {
        Some(u64::from_le_bytes(self.data.get(at..at + 8)?.try_into().ok()?))
    }
    /// A 2- or 4-byte index.
    fn index(&self, at: usize, wide: bool) -> Option<u32> {
        if wide { self.u32(at) } else { self.u16(at).map(u32::from) }
    }
}

/// Reads `bytes` (a whole DLL); None when it isn't a .NET assembly.
pub fn read(bytes: &[u8]) -> Option<AssemblyMetadata> {
    let r = Reader { data: bytes };
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe = r.u32(0x3C)? as usize;
    if bytes.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = r.u16(pe + 6)? as usize;
    let optional_size = r.u16(pe + 20)? as usize;
    let optional = pe + 24;
    let (count_at, directories) = match r.u16(optional)? {
        0x10B => (optional + 92, optional + 96),
        0x20B => (optional + 108, optional + 112),
        _ => return None,
    };
    if r.u32(count_at)? < 15 {
        return None;
    }
    let cli_rva = r.u32(directories + 14 * 8)? as usize;
    if cli_rva == 0 {
        return None;
    }
    let table = optional + optional_size;
    let offset_of = |rva: usize| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let (size, address, raw_size, raw) =
                (r.u32(s + 8)? as usize, r.u32(s + 12)? as usize, r.u32(s + 16)? as usize, r.u32(s + 20)? as usize);
            (rva >= address && rva < address + size.max(raw_size)).then(|| rva - address + raw)
        })
    };
    let cli = offset_of(cli_rva)?;
    let root = offset_of(r.u32(cli + 8)? as usize)?;
    if r.u32(root)? != 0x424A_5342 {
        return None;
    }
    let version_length = r.u32(root + 12)? as usize;
    let mut at = root + 16 + version_length;
    let streams = r.u16(at + 2)? as usize;
    at += 4;
    let (mut tables, mut strings, mut blobs) = (None, None, None);
    for _ in 0..streams {
        let (offset, size) = (r.u32(at)? as usize, r.u32(at + 4)? as usize);
        let name_start = at + 8;
        let name_end = name_start + bytes.get(name_start..)?.iter().position(|&b| b == 0)?;
        let name = bytes.get(name_start..name_end)?;
        let stream = (root + offset, size);
        match name {
            b"#~" | b"#-" => tables = Some(stream),
            b"#Strings" => strings = Some(stream),
            b"#Blob" => blobs = Some(stream),
            _ => {}
        }
        at = name_start + (name.len() + 1).div_ceil(4) * 4;
    }
    let (tables, (strings, _), blobs) = (tables?.0, strings?, blobs.map_or(0, |b| b.0));

    let heaps = r.u8(tables + 6)?;
    let (wide_string, wide_guid, wide_blob) = (heaps & 1 != 0, heaps & 2 != 0, heaps & 4 != 0);
    let valid = r.u64(tables + 8)?;
    let mut rows = [0u32; 64];
    let mut at = tables + 24;
    for (i, count) in rows.iter_mut().enumerate() {
        if valid & (1 << i) != 0 {
            *count = r.u32(at)?;
            at += 4;
        }
    }
    if heaps & 0x40 != 0 {
        at += 4;
    }
    let size = |wide: bool| if wide { 4 } else { 2 };
    let (s, g, b) = (size(wide_string), size(wide_guid), size(wide_blob));
    let t = |table: usize| size(rows[table] >= 1 << 16);
    let coded = |tables: &[usize], bits: u32| size(tables.iter().any(|&i| rows[i] >= 1 << (16 - bits)));
    let type_def_or_ref = coded(&[2, 1, 0x1B], 2);
    let has_constant = coded(&[4, 8, 0x17], 2);
    let has_custom_attribute = coded(
        &[
            6, 4, 1, 2, 8, 9, 0x0A, 0, 0x0E, 0x17, 0x14, 0x11, 0x1A, 0x1B, 0x20, 0x23, 0x26, 0x27, 0x28, 0x2A, 0x2C,
            0x2B,
        ],
        5,
    );
    let has_field_marshal = coded(&[4, 8], 1);
    let has_decl_security = coded(&[2, 6, 0x20], 2);
    let member_ref_parent = coded(&[2, 1, 0x1A, 6, 0x1B], 3);
    let has_semantics = coded(&[0x14, 0x17], 1);
    let method_def_or_ref = coded(&[6, 0x0A], 1);
    let member_forwarded = coded(&[4, 6], 1);
    let custom_attribute_type = coded(&[6, 0x0A], 3);
    let resolution_scope = coded(&[0, 0x1A, 0x23, 1], 2);
    let row_size: [usize; 0x24] = [
        2 + s + 3 * g,                                    // Module
        resolution_scope + 2 * s,                         // TypeRef
        4 + 2 * s + type_def_or_ref + t(4) + t(6),        // TypeDef
        t(4),                                             // FieldPtr
        2 + s + b,                                        // Field
        t(6),                                             // MethodPtr
        8 + s + b + t(8),                                 // MethodDef
        t(8),                                             // ParamPtr
        4 + s,                                            // Param
        t(2) + type_def_or_ref,                           // InterfaceImpl
        member_ref_parent + s + b,                        // MemberRef
        2 + has_constant + b,                             // Constant
        has_custom_attribute + custom_attribute_type + b, // CustomAttribute
        has_field_marshal + b,                            // FieldMarshal
        2 + has_decl_security + b,                        // DeclSecurity
        6 + t(2),                                         // ClassLayout
        4 + t(4),                                         // FieldLayout
        b,                                                // StandAloneSig
        t(2) + t(0x14),                                   // EventMap
        t(0x14),                                          // EventPtr
        2 + s + type_def_or_ref,                          // Event
        t(2) + t(0x17),                                   // PropertyMap
        t(0x17),                                          // PropertyPtr
        2 + s + b,                                        // Property
        2 + t(6) + has_semantics,                         // MethodSemantics
        t(2) + 2 * method_def_or_ref,                     // MethodImpl
        s,                                                // ModuleRef
        b,                                                // TypeSpec
        2 + member_forwarded + s + t(0x1A),               // ImplMap
        4 + t(4),                                         // FieldRVA
        8,                                                // EncLog
        4,                                                // EncMap
        16 + b + 2 * s,                                   // Assembly
        4,                                                // AssemblyProcessor
        12,                                               // AssemblyOS
        12 + 2 * b + 2 * s,                               // AssemblyRef
    ];
    let mut start = [0usize; 0x24];
    for i in 0..0x24 {
        start[i] = at;
        at += row_size[i] * rows[i] as usize;
    }
    if rows[0x20] == 0 {
        return None; // a module, not an assembly
    }

    let string = |index: u32| -> Option<String> {
        let from = strings + index as usize;
        let len = bytes.get(from..)?.iter().position(|&c| c == 0)?;
        Some(String::from_utf8_lossy(&bytes[from..from + len]).into_owned())
    };
    let blob = |index: u32| -> Option<&[u8]> {
        let from = blobs + index as usize;
        let first = r.u8(from)?;
        let (len, header) = if first & 0x80 == 0 {
            (first as usize, 1)
        } else if first & 0xC0 == 0x80 {
            ((((first & 0x3F) as usize) << 8) | r.u8(from + 1)? as usize, 2)
        } else {
            (
                (((first & 0x1F) as usize) << 24)
                    | (r.u8(from + 1)? as usize) << 16
                    | (r.u8(from + 2)? as usize) << 8
                    | r.u8(from + 3)? as usize,
                4,
            )
        };
        bytes.get(from + header..from + header + len)
    };
    let version = |at: usize| -> Option<AssemblyVersion> {
        Some(AssemblyVersion(r.u16(at)?, r.u16(at + 2)?, r.u16(at + 4)?, r.u16(at + 6)?))
    };

    let a = start[0x20];
    let public_key = blob(r.index(a + 16, wide_blob)?)?.to_vec();
    let name = string(r.index(a + 16 + b, wide_string)?)?;
    let assembly_version = version(a + 4)?;

    let mut references = Vec::new();
    for i in 0..rows[0x23] as usize {
        let row = start[0x23] + i * row_size[0x23];
        let token = blob(r.index(row + 12, wide_blob)?)?;
        references.push(AssemblyReference {
            name: string(r.index(row + 12 + b, wide_string)?)?,
            version: version(row)?,
            strong_named: !token.is_empty(),
        });
    }
    Some(AssemblyMetadata { name, version: assembly_version, public_key, references })
}

/// The public key token of a public key: the last 8 bytes of its SHA-1, reversed, in lower-case hex.
pub fn public_key_token(public_key: &[u8]) -> Option<String> {
    if public_key.is_empty() {
        return None;
    }
    let hash = dnnm_infrastructure::sha1::sha1(public_key);
    Some(hash[12..].iter().rev().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_print() {
        assert_eq!(AssemblyVersion::parse("13.0.0.0"), Some(AssemblyVersion(13, 0, 0, 0)));
        assert_eq!(AssemblyVersion::parse("4.2"), Some(AssemblyVersion(4, 2, 0, 0)));
        assert_eq!(AssemblyVersion::parse("x"), None);
        assert_eq!(AssemblyVersion(9, 13, 2, 0).to_string(), "9.13.2.0");
    }

    #[test]
    fn a_native_dll_or_garbage_is_none() {
        assert_eq!(read(b"not a dll"), None);
        let kernel = std::fs::read(r"C:\Windows\System32\kernel32.dll");
        if let Ok(kernel) = kernel {
            assert_eq!(read(&kernel), None);
        }
    }

    #[test]
    fn reads_a_framework_assembly() {
        // .NET Framework's System.dll is on every Windows DNN Manager supports; skipped where it isn't.
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let path = std::path::Path::new(&windows).join(r"Microsoft.NET\Framework64\v4.0.30319\System.dll");
        let Ok(bytes) = std::fs::read(&path) else { return };
        let metadata = read(&bytes).expect("System.dll is an assembly");
        assert_eq!(metadata.name, "System");
        assert_eq!(metadata.version, AssemblyVersion(4, 0, 0, 0));
        assert_eq!(public_key_token(&metadata.public_key).as_deref(), Some("b77a5c561934e089"));
        let mscorlib = metadata.references.iter().find(|r| r.name == "mscorlib").expect("references mscorlib");
        assert_eq!(mscorlib.version, AssemblyVersion(4, 0, 0, 0));
        assert!(mscorlib.strong_named);
    }
}
