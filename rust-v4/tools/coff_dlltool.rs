//! Pure-Rust PE/COFF x86_64 Long Import Library generator (`dlltool` drop-in for `rustc -C dlltool`).
//! Emits genuine GNU-ld compatible x86_64 COFF import objects (head `.idata$2` + per-symbol
//! `.text` thunk / `.idata$4` / `.idata$5` / `.idata$6` / `.idata$7` + tail null terminator)
//! with unique per-archive head/tail symbols so multiple crates importing the same DLL via
//! `#[link(kind = "raw-dylib")]` NEVER collide across `.idata$4`/`.idata$5` terminators!

use std::env;
use std::fs;
use std::process;

const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_REL_AMD64_ADDR32NB: u16 = 0x0003;
const IMAGE_REL_AMD64_REL32: u16 = 0x0004;
const IMAGE_SYM_CLASS_EXTERNAL: u8 = 2;
const IMAGE_SYM_CLASS_STATIC: u8 = 3;
const IMAGE_SYM_CLASS_SECTION: u8 = 104;

const IMAGE_SCN_CNT_CODE: u32 = 0x0000_0020;
const IMAGE_SCN_CNT_INITIALIZED_DATA: u32 = 0x0000_0040;
const IMAGE_SCN_ALIGN_2BYTES: u32 = 0x0020_0000;
const IMAGE_SCN_ALIGN_4BYTES: u32 = 0x0030_0000;
const IMAGE_SCN_ALIGN_8BYTES: u32 = 0x0040_0000;
const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

struct ExportSym {
    name: String,
    export_as: Option<String>,
    ordinal: Option<u16>,
    noname: bool,
    is_data: bool,
}

fn parse_def(text: &str) -> (Option<String>, Vec<ExportSym>) {
    let mut lib_name: Option<String> = None;
    let mut exports = Vec::new();
    let mut in_exports = false;

    for raw_line in text.lines() {
        let line = raw_line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("LIBRARY") {
            let rest = line[7..].trim().trim_matches('"');
            if !rest.is_empty() {
                lib_name = Some(rest.to_string());
            }
            continue;
        }
        if upper == "EXPORTS" {
            in_exports = true;
            continue;
        }
        if !in_exports {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        let first = tokens[0].trim_matches('"');
        let (sym_name, export_as, mut rest_idx) = if tokens.len() >= 3 && tokens[1] == "==" {
            (
                first.to_string(),
                Some(tokens[2].trim_matches('"').to_string()),
                3usize,
            )
        } else if let Some((lhs, rhs)) = first.split_once("==") {
            (
                lhs.trim_matches('"').to_string(),
                Some(rhs.trim_matches('"').to_string()),
                1usize,
            )
        } else if let Some((lhs, rhs)) = first.split_once('=') {
            (
                lhs.trim_matches('"').to_string(),
                Some(rhs.trim_matches('"').to_string()),
                1usize,
            )
        } else {
            (first.to_string(), None, 1usize)
        };
        let mut ordinal = None;
        let mut noname = false;
        let mut is_data = false;
        while rest_idx < tokens.len() {
            let tok = tokens[rest_idx];
            if let Some(num) = tok.strip_prefix('@') {
                if let Ok(ord) = num.parse::<u16>() {
                    ordinal = Some(ord);
                }
            } else if tok.eq_ignore_ascii_case("NONAME") {
                noname = true;
            } else if tok.eq_ignore_ascii_case("DATA") {
                is_data = true;
            }
            rest_idx += 1;
        }
        if !sym_name.is_empty() {
            exports.push(ExportSym {
                name: sym_name,
                export_as,
                ordinal,
                noname,
                is_data,
            });
        }
    }
    (lib_name, exports)
}

fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_name8(buf: &mut Vec<u8>, name: &str) {
    let mut b = [0u8; 8];
    let bytes = name.as_bytes();
    let n = bytes.len().min(8);
    b[..n].copy_from_slice(&bytes[..n]);
    buf.extend_from_slice(&b);
}

struct CoffBuilder {
    sections: Vec<CoffSection>,
    symbols: Vec<CoffSymbol>,
    strtab: Vec<u8>,
}

struct CoffSection {
    name: String,
    data: Vec<u8>,
    relocs: Vec<(u32, u32, u16)>,
    chars: u32,
}

struct CoffSymbol {
    name: String,
    value: u32,
    section: i16,
    sym_type: u16,
    storage_class: u8,
}

impl CoffBuilder {
    fn new() -> Self {
        Self {
            sections: Vec::new(),
            symbols: Vec::new(),
            strtab: vec![0, 0, 0, 0],
        }
    }

    fn add_section(
        &mut self,
        name: &str,
        data: Vec<u8>,
        relocs: Vec<(u32, u32, u16)>,
        chars: u32,
    ) -> i16 {
        self.sections.push(CoffSection {
            name: name.to_string(),
            data,
            relocs,
            chars,
        });
        self.sections.len() as i16
    }

    fn add_symbol(&mut self, name: &str, value: u32, section: i16, storage_class: u8) -> u32 {
        let idx = self.symbols.len() as u32;
        self.symbols.push(CoffSymbol {
            name: name.to_string(),
            value,
            section,
            sym_type: 0,
            storage_class,
        });
        idx
    }

    fn encode_sym_name(&mut self, name: &str) -> [u8; 8] {
        let mut out = [0u8; 8];
        let bytes = name.as_bytes();
        if bytes.len() <= 8 {
            out[..bytes.len()].copy_from_slice(bytes);
        } else {
            let off = self.strtab.len() as u32;
            self.strtab.extend_from_slice(bytes);
            self.strtab.push(0);
            out[4..8].copy_from_slice(&off.to_le_bytes());
        }
        out
    }

    fn finish(mut self) -> Vec<u8> {
        let nsec = self.sections.len() as u16;
        let nsym = self.symbols.len() as u32;

        let mut encoded_names = Vec::with_capacity(self.symbols.len());
        let sym_names: Vec<String> = self.symbols.iter().map(|s| s.name.clone()).collect();
        for name in sym_names {
            encoded_names.push(self.encode_sym_name(&name));
        }
        let strtab_len = self.strtab.len() as u32;
        self.strtab[0..4].copy_from_slice(&strtab_len.to_le_bytes());

        let header_size = 20u32 + (nsec as u32) * 40u32;
        let mut cur_off = header_size;
        let mut sec_data_offs = Vec::new();
        let mut sec_reloc_offs = Vec::new();

        for sec in &self.sections {
            if sec.data.is_empty() {
                sec_data_offs.push(0u32);
            } else {
                sec_data_offs.push(cur_off);
                cur_off += sec.data.len() as u32;
            }
            if sec.relocs.is_empty() {
                sec_reloc_offs.push(0u32);
            } else {
                sec_reloc_offs.push(cur_off);
                cur_off += (sec.relocs.len() as u32) * 10u32;
            }
        }
        let symtab_off = cur_off;

        let mut out = Vec::with_capacity(symtab_off as usize + (nsym as usize) * 18 + self.strtab.len());
        push_u16(&mut out, IMAGE_FILE_MACHINE_AMD64);
        push_u16(&mut out, nsec);
        push_u32(&mut out, 0);
        push_u32(&mut out, symtab_off);
        push_u32(&mut out, nsym);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);

        for (i, sec) in self.sections.iter().enumerate() {
            push_name8(&mut out, &sec.name);
            push_u32(&mut out, 0);
            push_u32(&mut out, 0);
            push_u32(&mut out, sec.data.len() as u32);
            push_u32(&mut out, sec_data_offs[i]);
            push_u32(&mut out, sec_reloc_offs[i]);
            push_u32(&mut out, 0);
            push_u16(&mut out, sec.relocs.len() as u16);
            push_u16(&mut out, 0);
            push_u32(&mut out, sec.chars);
        }

        for sec in &self.sections {
            out.extend_from_slice(&sec.data);
            for &(vaddr, sym_idx, rtype) in &sec.relocs {
                push_u32(&mut out, vaddr);
                push_u32(&mut out, sym_idx);
                push_u16(&mut out, rtype);
            }
        }

        for (i, sym) in self.symbols.iter().enumerate() {
            out.extend_from_slice(&encoded_names[i]);
            push_u32(&mut out, sym.value);
            push_u16(&mut out, sym.section as u16);
            push_u16(&mut out, sym.sym_type);
            out.push(sym.storage_class);
            out.push(0);
        }

        out.extend_from_slice(&self.strtab);
        out
    }
}

fn fnv1a_64(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for s in parts {
        for &b in s.as_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Head `.obj`: defines `_head_<uid>` (`.idata$2` descriptor + `.idata$6` DLL name +
/// `.idata$4` ILT start anchor + `.idata$5` IAT start anchor) and pulls in `_tail_<uid>`.
fn build_head_obj(dll: &str, head_sym: &str, tail_sym: &str) -> Vec<u8> {
    let mut b = CoffBuilder::new();
    let mut dll_bytes = dll.as_bytes().to_vec();
    dll_bytes.push(0);
    if dll_bytes.len() % 2 != 0 {
        dll_bytes.push(0);
    }

    let _ = b.add_symbol(head_sym, 0, 1, IMAGE_SYM_CLASS_EXTERNAL);
    let _ = b.add_symbol(".idata$2", 0, 1, IMAGE_SYM_CLASS_SECTION);
    let s_idata6 = b.add_symbol(".idata$6", 0, 2, IMAGE_SYM_CLASS_STATIC);
    let s_idata4 = b.add_symbol(".idata$4", 0, 3, IMAGE_SYM_CLASS_STATIC);
    let s_idata5 = b.add_symbol(".idata$5", 0, 4, IMAGE_SYM_CLASS_STATIC);
    let s_tail = b.add_symbol(tail_sym, 0, 0, IMAGE_SYM_CLASS_EXTERNAL);

    let idata2_relocs = vec![
        (0u32, s_idata4, IMAGE_REL_AMD64_ADDR32NB),  // OriginalFirstThunk (ILT)
        (12u32, s_idata6, IMAGE_REL_AMD64_ADDR32NB), // Name RVA
        (16u32, s_idata5, IMAGE_REL_AMD64_ADDR32NB), // FirstThunk (IAT)
    ];
    b.add_section(
        ".idata$2",
        vec![0u8; 20],
        idata2_relocs,
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_4BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    b.add_section(
        ".idata$6",
        dll_bytes,
        Vec::new(),
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_2BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    b.add_section(
        ".idata$4",
        Vec::new(),
        Vec::new(),
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_8BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    b.add_section(
        ".idata$5",
        Vec::new(),
        Vec::new(),
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_8BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    // Reference `_tail_<uid>` via a real relocation in `.idata$7` so ld ALWAYS pulls in tail.o!
    b.add_section(
        ".idata$7",
        vec![0u8; 4],
        vec![(0u32, s_tail, IMAGE_REL_AMD64_ADDR32NB)],
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_4BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    b.finish()
}

/// Tail `.obj`: defines `_tail_<uid>` with 8 zero bytes in `.idata$4` & `.idata$5`
/// terminating this archive's ILT & IAT cleanly!
fn build_tail_obj(tail_sym: &str) -> Vec<u8> {
    let mut b = CoffBuilder::new();
    b.add_section(
        ".idata$4",
        vec![0u8; 8],
        Vec::new(),
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_8BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    b.add_section(
        ".idata$5",
        vec![0u8; 8],
        Vec::new(),
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_8BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    b.add_symbol(tail_sym, 0, 1, IMAGE_SYM_CLASS_EXTERNAL);
    b.finish()
}

/// Long COFF Import `.obj` for one symbol: `.text` (`jmp [rip + __imp_sym]`), `.idata$4`,
/// `.idata$5` (`__imp_sym`), `.idata$6` (hint+name), and `.idata$7` referencing `_head_<uid>`.
fn build_symbol_obj(exp: &ExportSym, head_sym: &str) -> Vec<u8> {
    let mut b = CoffBuilder::new();
    let dll_export_name = exp.export_as.as_deref().unwrap_or(&exp.name);
    let imp_sym_name = format!("__imp_{}", exp.name);

    // Build .idata$6 hint (u16) + NUL-terminated export name (even-padded)
    let mut hint_name = Vec::new();
    if !exp.noname {
        push_u16(&mut hint_name, exp.ordinal.unwrap_or(0));
        hint_name.extend_from_slice(dll_export_name.as_bytes());
        hint_name.push(0);
        if hint_name.len() % 2 != 0 {
            hint_name.push(0);
        }
    }

    // Section indices:
    // If code: 1=.text, 2=.idata$4, 3=.idata$5, 4=.idata$6 (if !noname), last=.idata$7
    // If data: 1=.idata$4, 2=.idata$5, 3=.idata$6 (if !noname), last=.idata$7
    let text_sec = if !exp.is_data { Some(1i16) } else { None };
    let idata4_sec = if text_sec.is_some() { 2i16 } else { 1i16 };
    let idata5_sec = idata4_sec + 1;
    let idata6_sec = if !exp.noname { Some(idata5_sec + 1) } else { None };

    if let Some(ts) = text_sec {
        let _ = b.add_symbol(&exp.name, 0, ts, IMAGE_SYM_CLASS_EXTERNAL);
    }
    let s_imp = b.add_symbol(&imp_sym_name, 0, idata5_sec, IMAGE_SYM_CLASS_EXTERNAL);
    let s_idata6 = idata6_sec.map(|sec6| b.add_symbol(".idata$6", 0, sec6, IMAGE_SYM_CLASS_STATIC));
    let s_head = b.add_symbol(head_sym, 0, 0, IMAGE_SYM_CLASS_EXTERNAL);

    if text_sec.is_some() {
        // x86_64 RIP-relative indirect jump: FF 25 <rel32> -> jmp qword ptr [rip + __imp_sym]
        b.add_section(
            ".text",
            vec![0xFF, 0x25, 0x00, 0x00, 0x00, 0x00],
            vec![(2u32, s_imp, IMAGE_REL_AMD64_REL32)],
            IMAGE_SCN_CNT_CODE | IMAGE_SCN_ALIGN_4BYTES | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
        );
    }

    let (thunk_data, thunk_relocs) = if let Some(sym6) = s_idata6 {
        (vec![0u8; 8], vec![(0u32, sym6, IMAGE_REL_AMD64_ADDR32NB)])
    } else {
        let ord = 0x8000_0000_0000_0000u64 | u64::from(exp.ordinal.unwrap_or(0));
        (ord.to_le_bytes().to_vec(), Vec::new())
    };

    b.add_section(
        ".idata$4",
        thunk_data.clone(),
        thunk_relocs.clone(),
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_8BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    b.add_section(
        ".idata$5",
        thunk_data,
        thunk_relocs,
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_8BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );
    if idata6_sec.is_some() {
        b.add_section(
            ".idata$6",
            hint_name,
            Vec::new(),
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_2BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
        );
    }
    // Reference `_head_<uid>` via real relocation in `.idata$7` so ld ALWAYS pulls in head.o!
    b.add_section(
        ".idata$7",
        vec![0u8; 4],
        vec![(0u32, s_head, IMAGE_REL_AMD64_ADDR32NB)],
        IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_4BYTES | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
    );

    b.finish()
}

fn write_ar_header(out: &mut Vec<u8>, name: &str, size: usize) {
    let mut hdr = [b' '; 60];
    let nb = name.as_bytes();
    let n = nb.len().min(16);
    hdr[..n].copy_from_slice(&nb[..n]);
    hdr[16..17].copy_from_slice(b"0");
    hdr[28..29].copy_from_slice(b"0");
    hdr[34..35].copy_from_slice(b"0");
    hdr[40..46].copy_from_slice(b"100644");
    let sz_str = size.to_string();
    let sb = sz_str.as_bytes();
    hdr[48..48 + sb.len().min(10)].copy_from_slice(&sb[..sb.len().min(10)]);
    hdr[58] = b'`';
    hdr[59] = b'\n';
    out.extend_from_slice(&hdr);
}

fn build_import_lib(dll: &str, out_lib: &str, exports: &[ExportSym]) -> Vec<u8> {
    let mut hash_parts: Vec<&str> = vec![dll, out_lib];
    for e in exports {
        hash_parts.push(&e.name);
    }
    let uid = fnv1a_64(&hash_parts);
    let pfx = format!("u{:06x}", uid & 0x00FF_FFFF);
    let head_sym = format!("_head_{uid:016x}");
    let tail_sym = format!("_tail_{uid:016x}");

    let mut members: Vec<(String, Vec<u8>, Vec<String>)> = Vec::with_capacity(2 + exports.len());

    // Alphabetical order within archive AND globally across archives:
    // u<6hex>a.o (head) < u<6hex>s<4hex>.o (symbols) < u<6hex>z.o (tail null terminator)
    members.push((
        format!("{pfx}a.o/"),
        build_head_obj(dll, &head_sym, &tail_sym),
        vec![head_sym.clone()],
    ));

    for (idx, exp) in exports.iter().enumerate() {
        let mut syms = vec![format!("__imp_{}", exp.name)];
        if !exp.is_data {
            syms.push(exp.name.clone());
        }
        members.push((
            format!("{pfx}s{:04x}.o/", idx + 1),
            build_symbol_obj(exp, &head_sym),
            syms,
        ));
    }

    members.push((
        format!("{pfx}z.o/"),
        build_tail_obj(&tail_sym),
        vec![tail_sym.clone()],
    ));

    let mut sym_entries: Vec<(String, usize)> = Vec::new();
    for (mi, (_, _, syms)) in members.iter().enumerate() {
        for s in syms {
            sym_entries.push((s.clone(), mi));
        }
    }

    let sym_strings_len: usize = sym_entries.iter().map(|(s, _)| s.len() + 1).sum();
    let lm1_size = 4 + sym_entries.len() * 4 + sym_strings_len;
    let lm1_padded = lm1_size + (lm1_size & 1);

    let mut sorted_entries = sym_entries.clone();
    sorted_entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let lm2_size = 4 + members.len() * 4 + 4 + sorted_entries.len() * 2 + sym_strings_len;
    let lm2_padded = lm2_size + (lm2_size & 1);

    let mut offset = 8usize + 60 + lm1_padded + 60 + lm2_padded;
    let mut member_offsets = Vec::with_capacity(members.len());
    for (_, data, _) in &members {
        member_offsets.push(offset as u32);
        let padded = data.len() + (data.len() & 1);
        offset += 60 + padded;
    }

    let mut out = Vec::with_capacity(offset);
    out.extend_from_slice(b"!<arch>\n");

    write_ar_header(&mut out, "/", lm1_size);
    out.extend_from_slice(&(sym_entries.len() as u32).to_be_bytes());
    for (_, mi) in &sym_entries {
        out.extend_from_slice(&member_offsets[*mi].to_be_bytes());
    }
    for (s, _) in &sym_entries {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    if lm1_size & 1 != 0 {
        out.push(b'\n');
    }

    write_ar_header(&mut out, "/", lm2_size);
    out.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for &mo in &member_offsets {
        out.extend_from_slice(&mo.to_le_bytes());
    }
    out.extend_from_slice(&(sorted_entries.len() as u32).to_le_bytes());
    for (_, mi) in &sorted_entries {
        out.extend_from_slice(&((*mi as u16) + 1).to_le_bytes());
    }
    for (s, _) in &sorted_entries {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    if lm2_size & 1 != 0 {
        out.push(b'\n');
    }

    for (name, data, _) in &members {
        write_ar_header(&mut out, name, data.len());
        out.extend_from_slice(data);
        if data.len() & 1 != 0 {
            out.push(b'\n');
        }
    }

    out
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut def_path = String::new();
    let mut dll_name = String::new();
    let mut out_lib = String::new();

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "-d" | "--input-def" => {
                if i + 1 < args.len() {
                    def_path = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            "-D" | "--dllname" => {
                if i + 1 < args.len() {
                    dll_name = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            "-l" | "--output-lib" => {
                if i + 1 < args.len() {
                    out_lib = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            "-m" | "-f" | "--temp-prefix" | "-A" => {
                i += 2;
                continue;
            }
            _ => {}
        }
        i += 1;
    }

    if def_path.is_empty() || out_lib.is_empty() {
        eprintln!("coff-dlltool: missing -d <def> or -l <lib>");
        process::exit(1);
    }

    let def_text = fs::read_to_string(&def_path).unwrap_or_else(|e| {
        eprintln!("coff-dlltool: failed to read {def_path}: {e}");
        process::exit(1);
    });

    let (parsed_dll, exports) = parse_def(&def_text);
    let final_dll = if !dll_name.is_empty() {
        dll_name
    } else {
        parsed_dll.unwrap_or_else(|| "unknown.dll".to_string())
    };

    let archive = build_import_lib(&final_dll, &out_lib, &exports);
    if let Err(e) = fs::write(&out_lib, &archive) {
        eprintln!("coff-dlltool: failed to write {out_lib}: {e}");
        process::exit(1);
    }
}
