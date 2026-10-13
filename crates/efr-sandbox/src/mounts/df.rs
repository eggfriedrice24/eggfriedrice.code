//! The `df` table of a snapshot, in GNU `df`'s format (coreutils 9).
//!
//! The renderer takes `df`'s options and no operand. It follows GNU's rules: without
//! `-a` it hides the dummy types, the file systems with no blocks and every mount of a
//! device but one (a real device name first, then the target nearer the root); `-l`
//! hides the remote types; sizes round up. Anything else, such as an operand, `--help`
//! or an option it does not know, gives `None`, and the child shell then runs the real
//! `df`.

use std::collections::HashMap;
use std::ffi::OsString;

use super::human::{Human, human_readable, percent};
use super::{MountEntry, MountSizes, MountSnapshot, is_dummy};

/// One column of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Source,
    Fstype,
    Itotal,
    Iused,
    Iavail,
    Ipcent,
    Size,
    Used,
    Avail,
    Pcent,
    File,
    Target,
}

impl Field {
    /// Every field of `--output` with no list, in its order.
    const ALL: [Field; 12] = [
        Field::Source,
        Field::Fstype,
        Field::Itotal,
        Field::Iused,
        Field::Iavail,
        Field::Ipcent,
        Field::Size,
        Field::Used,
        Field::Avail,
        Field::Pcent,
        Field::File,
        Field::Target,
    ];

    fn by_name(name: &str) -> Option<Field> {
        Some(match name {
            "source" => Field::Source,
            "fstype" => Field::Fstype,
            "itotal" => Field::Itotal,
            "iused" => Field::Iused,
            "iavail" => Field::Iavail,
            "ipcent" => Field::Ipcent,
            "size" => Field::Size,
            "used" => Field::Used,
            "avail" => Field::Avail,
            "pcent" => Field::Pcent,
            "file" => Field::File,
            "target" => Field::Target,
            _ => return None,
        })
    }

    /// The narrowest the column is.
    fn min_width(self) -> usize {
        match self {
            Field::Source => 14,
            Field::Fstype | Field::Ipcent | Field::Pcent => 4,
            Field::Itotal
            | Field::Iused
            | Field::Iavail
            | Field::Size
            | Field::Used
            | Field::Avail => 5,
            Field::File | Field::Target => 0,
        }
    }

    fn left(self) -> bool {
        matches!(self, Field::Source | Field::Fstype | Field::File | Field::Target)
    }

    fn inodes(self) -> bool {
        matches!(self, Field::Itotal | Field::Iused | Field::Iavail | Field::Ipcent)
    }
}

/// Which header `df` prints, as its `header_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Default,
    Inodes,
    Human,
    Posix,
    Output,
}

/// The options of one `df` call.
#[derive(Debug, Clone)]
struct Options {
    all: bool,
    local: bool,
    inodes: bool,
    print_type: bool,
    posix: bool,
    total: bool,
    output: Option<Vec<Field>>,
    human: Human,
    block_size: u64,
    types: Vec<String>,
    excludes: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            all: false,
            local: false,
            inodes: false,
            print_type: false,
            posix: false,
            total: false,
            output: None,
            human: Human::default(),
            block_size: 1024,
            types: Vec::new(),
            excludes: Vec::new(),
        }
    }
}

/// What a long option of `df` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Long {
    All,
    BlockSize,
    Inodes,
    HumanReadable,
    Si,
    Local,
    Output,
    Portability,
    PrintType,
    Sync,
    NoSync,
    Total,
    Type,
    ExcludeType,
    /// `--help` and `--version`: the real `df` answers them.
    Other,
}

/// `df`'s long options. A unique prefix names one, as `getopt_long` reads them.
const LONG: &[(&str, Long)] = &[
    ("all", Long::All),
    ("block-size", Long::BlockSize),
    ("inodes", Long::Inodes),
    ("human-readable", Long::HumanReadable),
    ("si", Long::Si),
    ("local", Long::Local),
    ("output", Long::Output),
    ("portability", Long::Portability),
    ("print-type", Long::PrintType),
    ("sync", Long::Sync),
    ("no-sync", Long::NoSync),
    ("total", Long::Total),
    ("type", Long::Type),
    ("exclude-type", Long::ExcludeType),
    ("help", Long::Other),
    ("version", Long::Other),
];

/// The `df` table of `entries` with the sizes of `snapshot`, for the options `args`,
/// or `None` when `args` hold an operand or an option that the table cannot follow.
pub fn render_df(
    entries: &[MountEntry],
    snapshot: &MountSnapshot,
    args: &[OsString],
) -> Option<String> {
    let options = parse(args)?;
    let mode = if options.output.is_some() {
        Mode::Output
    } else if options.inodes {
        Mode::Inodes
    } else if options.human.autoscale {
        Mode::Human
    } else if options.posix {
        Mode::Posix
    } else {
        Mode::Default
    };
    let columns = columns(&options, mode);
    let rows = rows(entries, snapshot, &options);
    if rows.is_empty() {
        return None;
    }
    let mut table = vec![columns.iter().map(|field| header(*field, &options, mode)).collect()];
    let mut grand = Values::zero();
    for row in &rows {
        table.push(columns.iter().map(|field| cell(*field, row, &options)).collect());
        grand.add(&row.values);
    }
    if options.total {
        let total = Row {
            source: "total".to_owned(),
            fstype: "-".to_owned(),
            target: if columns.contains(&Field::Source) { "-" } else { "total" }.to_owned(),
            values: grand,
        };
        table.push(columns.iter().map(|field| cell(*field, &total, &options)).collect());
    }
    Some(layout(&columns, &table))
}

/// The options of `args`, or `None` for an operand or an option that the table does
/// not follow.
fn parse(args: &[OsString]) -> Option<Options> {
    let mut options = Options::default();
    let mut words = args.iter().map(|arg| arg.to_str());
    while let Some(word) = words.next() {
        let word = word?;
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            let what = long_option(name)?;
            let needs_value = matches!(what, Long::BlockSize | Long::Type | Long::ExcludeType);
            let value = match (needs_value, value) {
                (true, Some(value)) => Some(value.to_owned()),
                (true, None) => Some(words.next()??.to_owned()),
                (false, Some(_)) if what != Long::Output => return None,
                (false, value) => value.map(str::to_owned),
            };
            apply_long(&mut options, what, value)?;
        } else {
            // An operand, `-` or `--` gives `None`: the real `df` reads the file system
            // of a path.
            let cluster = word.strip_prefix('-').filter(|rest| !rest.is_empty())?;
            for (at, letter) in cluster.char_indices() {
                if matches!(letter, 'B' | 't' | 'x' | 'F') {
                    let rest = &cluster[at + letter.len_utf8()..];
                    let value =
                        if rest.is_empty() { words.next()??.to_owned() } else { rest.to_owned() };
                    apply_short(&mut options, letter, Some(value))?;
                    break;
                }
                apply_short(&mut options, letter, None)?;
            }
        }
    }
    let both = options.types.iter().any(|kind| options.excludes.contains(kind));
    (!both).then_some(options)
}

fn long_option(name: &str) -> Option<Long> {
    if let Some((_, what)) = LONG.iter().find(|(long, _)| *long == name) {
        return Some(*what);
    }
    let mut found = LONG.iter().filter(|(long, _)| long.starts_with(name) && !name.is_empty());
    let first = found.next()?;
    found.next().is_none().then_some(first.1)
}

fn apply_long(options: &mut Options, what: Long, value: Option<String>) -> Option<()> {
    match what {
        Long::All => apply_short(options, 'a', None),
        Long::BlockSize => apply_short(options, 'B', value),
        Long::Inodes => apply_short(options, 'i', None),
        Long::HumanReadable => apply_short(options, 'h', None),
        Long::Si => apply_short(options, 'H', None),
        Long::Local => apply_short(options, 'l', None),
        Long::Portability => apply_short(options, 'P', None),
        Long::PrintType => apply_short(options, 'T', None),
        Long::Type => apply_short(options, 't', value),
        Long::ExcludeType => apply_short(options, 'x', value),
        Long::Total => {
            options.total = true;
            Some(())
        }
        Long::Sync | Long::NoSync => Some(()),
        Long::Output => {
            // NOTE: df refuses --output with -i, -P or -T.
            if options.inodes || options.posix || options.print_type || options.output.is_some() {
                return None;
            }
            options.output = Some(match value {
                None => Field::ALL.to_vec(),
                Some(list) => {
                    let mut fields: Vec<Field> = Vec::new();
                    for name in list.split(',') {
                        let field = Field::by_name(name)?;
                        if fields.contains(&field) {
                            return None;
                        }
                        fields.push(field);
                    }
                    fields
                }
            });
            Some(())
        }
        Long::Other => None,
    }
}

fn apply_short(options: &mut Options, letter: char, value: Option<String>) -> Option<()> {
    let refuses_output = matches!(letter, 'i' | 'P' | 'T');
    if refuses_output && options.output.is_some() {
        return None;
    }
    match letter {
        'a' => options.all = true,
        'i' => options.inodes = true,
        'h' => {
            options.human =
                Human { autoscale: true, si: true, base_1024: true, ..Human::default() };
            options.block_size = 1;
        }
        'H' => {
            options.human = Human { autoscale: true, si: true, ..Human::default() };
            options.block_size = 1;
        }
        'k' => {
            options.human = Human::default();
            options.block_size = 1024;
        }
        'm' => {
            options.human = Human::default();
            options.block_size = 1024 * 1024;
        }
        'B' => {
            let (human, size) = block_size(&value?)?;
            options.human = human;
            options.block_size = size;
        }
        'l' => options.local = true,
        'P' => options.posix = true,
        'T' => options.print_type = true,
        't' | 'F' => options.types.push(value?),
        'x' => options.excludes.push(value?),
        'v' => {}
        _ => return None,
    }
    Some(())
}

/// A `--block-size` value, as gnulib's `human_options` reads it: `human-readable`,
/// `si`, or a number with an optional suffix such as `1K`, `1MB` or `MiB`. A suffix
/// with no number also prints the suffix after each size.
fn block_size(spec: &str) -> Option<(Human, u64)> {
    match spec {
        "human-readable" => {
            return Some((
                Human { autoscale: true, si: true, base_1024: true, ..Human::default() },
                1,
            ));
        }
        "si" => return Some((Human { autoscale: true, si: true, ..Human::default() }, 1)),
        _ => {}
    }
    let digits = spec.bytes().take_while(u8::is_ascii_digit).count();
    let (number, suffix) = spec.split_at(digits);
    let number: u64 = if number.is_empty() { 1 } else { number.parse().ok()? };
    let mut human = Human::default();
    let size = if suffix.is_empty() {
        number
    } else {
        let mut chars = suffix.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let exponent =
            u32::try_from(b"KMGTPEZYRQ".iter().position(|known| char::from(*known) == letter)? + 1)
                .ok()?;
        let base: u64 = match chars.as_str() {
            "" | "iB" => 1024,
            "B" => 1000,
            _ => return None,
        };
        if number == 1 && digits == 0 {
            human.si = true;
            human.bytes = suffix.ends_with('B');
            human.base_1024 = !suffix.ends_with('B') || suffix.ends_with("iB");
        }
        number.checked_mul(base.checked_pow(exponent)?)?
    };
    (size > 0).then_some((human, size))
}

/// The values of one row: `None` for a value that is not known.
#[derive(Debug, Clone, Copy)]
struct Values {
    /// Bytes per block of the block values.
    unit: u64,
    blocks: Option<u64>,
    bfree: Option<u64>,
    bavail: Option<u64>,
    files: Option<u64>,
    ffree: Option<u64>,
}

impl Values {
    fn zero() -> Values {
        Values {
            unit: 1,
            blocks: Some(0),
            bfree: Some(0),
            bavail: Some(0),
            files: Some(0),
            ffree: Some(0),
        }
    }

    fn unknown() -> Values {
        Values { unit: 1, blocks: None, bfree: None, bavail: None, files: None, ffree: None }
    }

    /// Adds `other` to this total, in bytes.
    fn add(&mut self, other: &Values) {
        let bytes = |value: Option<u64>| value.map(|value| value.saturating_mul(other.unit));
        let sum = |into: &mut Option<u64>, value: Option<u64>| {
            if let (Some(into), Some(value)) = (into.as_mut(), value) {
                *into = into.saturating_add(value);
            }
        };
        sum(&mut self.blocks, bytes(other.blocks));
        sum(&mut self.bfree, bytes(other.bfree));
        sum(&mut self.bavail, bytes(other.bavail));
        sum(&mut self.files, other.files);
        sum(&mut self.ffree, other.ffree);
    }
}

/// One row of the table.
#[derive(Debug, Clone)]
struct Row {
    source: String,
    fstype: String,
    target: String,
    values: Values,
}

/// The fields of the table, in their order.
fn columns(options: &Options, mode: Mode) -> Vec<Field> {
    if let Some(fields) = &options.output {
        return fields.clone();
    }
    let mut fields = vec![Field::Source];
    if options.print_type {
        fields.push(Field::Fstype);
    }
    if mode == Mode::Inodes {
        fields.extend([Field::Itotal, Field::Iused, Field::Iavail, Field::Ipcent]);
    } else {
        fields.extend([Field::Size, Field::Used, Field::Avail, Field::Pcent]);
    }
    fields.push(Field::Target);
    fields
}

fn header(field: Field, options: &Options, mode: Mode) -> String {
    match field {
        Field::Source => "Filesystem".to_owned(),
        Field::Fstype => "Type".to_owned(),
        Field::Itotal => "Inodes".to_owned(),
        Field::Iused => "IUsed".to_owned(),
        Field::Iavail => "IFree".to_owned(),
        Field::Ipcent => "IUse%".to_owned(),
        Field::Size => match mode {
            Mode::Default => blocks_header(options),
            Mode::Output if !options.human.autoscale => blocks_header(options),
            Mode::Posix => format!("{}-blocks", options.block_size),
            _ => "Size".to_owned(),
        },
        Field::Used => "Used".to_owned(),
        Field::Avail => {
            if matches!(mode, Mode::Default | Mode::Posix) { "Available" } else { "Avail" }
                .to_owned()
        }
        Field::Pcent => if mode == Mode::Posix { "Capacity" } else { "Use%" }.to_owned(),
        Field::File => "File".to_owned(),
        Field::Target => "Mounted on".to_owned(),
    }
}

/// The `1K-blocks` header: the block size in the base that divides it more exactly.
fn blocks_header(options: &Options) -> String {
    let size = options.block_size;
    let (mut by_1000, mut by_1024) = (size, size);
    let (mut divides_1000, mut divides_1024);
    loop {
        divides_1000 = by_1000 % 1000 == 0;
        by_1000 /= 1000;
        divides_1024 = by_1024 % 1024 == 0;
        by_1024 /= 1024;
        if !(divides_1000 && divides_1024) {
            break;
        }
    }
    let mut human = Human {
        autoscale: true,
        si: true,
        suppress_point_zero: true,
        base_1024: options.human.base_1024,
        bytes: options.human.bytes,
    };
    if !divides_1000 && divides_1024 {
        human.base_1024 = true;
    }
    if !divides_1024 && divides_1000 {
        human.base_1024 = false;
    }
    if !human.base_1024 {
        human.bytes = true;
    }
    format!("{}-blocks", human_readable(size, human, 1, 1))
}

fn cell(field: Field, row: &Row, options: &Options) -> String {
    let values = &row.values;
    let number = |value: Option<u64>, field: Field| match value {
        None => "-".to_owned(),
        Some(value) if field.inodes() => human_readable(value, options.human, 1, 1),
        Some(value) => human_readable(value, options.human, values.unit, options.block_size),
    };
    let used = |total: Option<u64>, free: Option<u64>| Some(total?.saturating_sub(free?));
    match field {
        Field::Source => row.source.clone(),
        Field::Fstype => row.fstype.clone(),
        Field::Target => row.target.clone(),
        Field::File => "-".to_owned(),
        Field::Size => number(values.blocks, field),
        Field::Used => number(used(values.blocks, values.bfree), field),
        Field::Avail => number(values.bavail, field),
        Field::Pcent => match (used(values.blocks, values.bfree), values.bavail) {
            (Some(used), Some(avail)) => percent(used, avail),
            _ => "-".to_owned(),
        },
        Field::Itotal => number(values.files, field),
        Field::Iused => number(used(values.files, values.ffree), field),
        Field::Iavail => number(values.ffree, field),
        Field::Ipcent => match (used(values.files, values.ffree), values.ffree) {
            (Some(used), Some(free)) => percent(used, free),
            _ => "-".to_owned(),
        },
    }
}

/// gnulib's `ME_REMOTE`: a source with `host:`, a `//host` share, or a cluster type.
fn is_remote(entry: &MountEntry) -> bool {
    const REMOTE: &[&str] =
        &["acfs", "afs", "coda", "auristorfs", "fhgfs", "gpfs", "ibrix", "ocfs2", "vxfs"];
    let share = entry.source.starts_with("//")
        && matches!(entry.fstype.as_str(), "smbfs" | "smb3" | "cifs");
    entry.source.contains(':')
        || share
        || REMOTE.contains(&entry.fstype.as_str())
        || entry.source == "-hosts"
}

/// The rows that `df` prints, in its order.
fn rows(entries: &[MountEntry], snapshot: &MountSnapshot, options: &Options) -> Vec<Row> {
    let selected = |entry: &MountEntry| {
        (options.types.is_empty() || options.types.contains(&entry.fstype))
            && !options.excludes.contains(&entry.fstype)
    };
    let listed: Vec<&MountEntry> = if options.all {
        entries.iter().collect()
    } else {
        one_per_device(entries, options, &selected)
    };
    let mut rows = Vec::new();
    for entry in listed {
        if (options.local && is_remote(entry))
            || (!options.all && is_dummy(&entry.fstype))
            || !selected(entry)
        {
            continue;
        }
        let mut fstype = entry.fstype.clone();
        let values = match snapshot.sizes(entry.id) {
            Some(MountSizes::Measured { blocks, bfree, bavail, frsize, files, ffree }) => Values {
                unit: (*frsize).max(1),
                blocks: Some(*blocks),
                bfree: Some(*bfree),
                bavail: Some(*bavail),
                files: Some(*files),
                ffree: Some(*ffree),
            },
            // NOTE: df prints an automount point with -a as one it cannot read: it never
            // triggers the mount.
            Some(MountSizes::Dummy) if entry.fstype != "autofs" => Values::zero(),
            Some(MountSizes::Dummy) => {
                fstype = "-".to_owned();
                Values::unknown()
            }
            Some(MountSizes::Failed { errno }) if matches!(*errno, ENOENT | EACCES) => {
                if !options.all {
                    continue;
                }
                fstype = "-".to_owned();
                Values::unknown()
            }
            _ => Values::unknown(),
        };
        if values.blocks == Some(0) && !options.all {
            continue;
        }
        rows.push(Row {
            source: display(&entry.source),
            fstype,
            target: display(&entry.target.to_string_lossy()),
            values,
        });
    }
    rows
}

const ENOENT: i32 = 2;
const EACCES: i32 = 13;

/// GNU `df`'s `filter_mount_list`: one mount of each device, the others dropped. The
/// mounts that a later step hides still take part, as in `df`.
fn one_per_device<'a>(
    entries: &'a [MountEntry],
    options: &Options,
    selected: &dyn Fn(&MountEntry) -> bool,
) -> Vec<&'a MountEntry> {
    let mut list: Vec<&MountEntry> = Vec::new();
    let mut seen: HashMap<(u32, u32), usize> = HashMap::new();
    for entry in entries {
        let skip =
            (options.local && is_remote(entry)) || is_dummy(&entry.fstype) || !selected(entry);
        if !skip && let Some(&at) = seen.get(&entry.device()) {
            let kept = list[at];
            let nearer_root = kept.target.as_os_str().len() > entry.target.as_os_str().len();
            let source_below_root = kept.root.as_os_str().len() < entry.root.as_os_str().len();
            let replace = if !options.total
                && is_remote(entry)
                && is_remote(kept)
                && kept.source != entry.source
            {
                None
            } else if (entry.source.contains('/') && !kept.source.contains('/'))
                || (nearer_root && !source_below_root)
                || (kept.source != entry.source && kept.target == entry.target)
            {
                Some(true)
            } else {
                Some(false)
            };
            match replace {
                Some(true) => {
                    list[at] = entry;
                    continue;
                }
                Some(false) => continue,
                None => {}
            }
        }
        seen.insert(entry.device(), list.len());
        list.push(entry);
    }
    list
}

/// A name as `df` prints it: control characters as `?`.
fn display(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

/// The table, each column as wide as its widest cell, the last one not padded.
fn layout(columns: &[Field], table: &[Vec<String>]) -> String {
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(at, field)| {
            table
                .iter()
                .map(|row| row[at].chars().count())
                .max()
                .unwrap_or(0)
                .max(field.min_width())
        })
        .collect();
    let mut out = String::new();
    for row in table {
        let mut line = String::new();
        for (at, field) in columns.iter().enumerate() {
            if at > 0 {
                line.push(' ');
            }
            let text = &row[at];
            let pad = widths[at].saturating_sub(text.chars().count());
            let last = at + 1 == columns.len();
            if field.left() {
                line.push_str(text);
                if !last {
                    line.extend(std::iter::repeat_n(' ', pad));
                }
            } else {
                line.extend(std::iter::repeat_n(' ', pad));
                line.push_str(text);
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}
