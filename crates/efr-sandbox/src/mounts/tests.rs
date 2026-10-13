//! The mount table, the rules of the snapshot, and the `df` and `findmnt --df` tables.
//!
//! The golden tables were checked against GNU `df` (coreutils 9.12) and `findmnt`
//! (util-linux 2.42.4): the renderer printed the same table as the real programs for
//! the host's own mounts with each of these options, except for the use of the mounts
//! that changed between the two reads.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::*;

/// A host's table: a bind mount of `/home`, the dummy types, an automount point, a
/// remote FUSE mount with a space in its target, an NFS mount that did not answer, a
/// mount below the masked runtime dir, another user's runtime dir and a mount whose
/// target is gone.
const TABLE: &str = "\
21 1 259:3 / / rw,relatime shared:1 - ext4 /dev/nvme1n1p3 rw
22 21 0:5 / /dev rw,nosuid shared:2 - devtmpfs devtmpfs rw,size=32379608k
23 22 0:24 / /dev/shm rw,nosuid,nodev shared:3 - tmpfs tmpfs rw
24 21 0:22 / /proc rw,nosuid,nodev,noexec,relatime shared:4 - proc proc rw
25 21 0:23 / /sys rw,nosuid,nodev,noexec,relatime shared:5 - sysfs sysfs rw
26 25 0:27 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime shared:6 - cgroup2 cgroup2 rw
27 24 0:35 / /proc/sys/fs/binfmt_misc rw,relatime shared:7 - autofs systemd-1 rw,fd=37
28 21 0:30 / /tmp rw,nosuid,nodev shared:8 - tmpfs tmpfs rw,size=32456332k
29 21 259:1 / /home rw,relatime shared:9 - ext4 /dev/nvme0n1p1 rw
30 21 259:1 /me/data /srv/data rw,relatime shared:9 - ext4 /dev/nvme0n1p1 rw
31 21 0:40 / /mnt/my\\040disk rw,relatime shared:10 - fuse.sshfs host:/export rw,user_id=1000,group_id=1000
32 21 0:41 / /mnt/nas rw,relatime shared:11 - nfs4 nas:/vol rw
33 21 0:42 / /run/user/1000 rw,nosuid,nodev,relatime shared:12 - tmpfs tmpfs rw,size=6492600k,mode=700,uid=1000
34 33 0:43 / /run/user/1000/gvfs rw,nosuid,nodev,relatime shared:13 - fuse.gvfsd-fuse gvfsd-fuse rw,user_id=1000,group_id=1000
35 21 0:44 / /run/user/1001 rw,nosuid,nodev,relatime shared:14 - tmpfs tmpfs rw,uid=1001
36 21 259:2 / /boot rw,relatime shared:15 - vfat /dev/nvme1n1p1 rw
37 21 8:1 / /media/gone rw,relatime shared:16 - ext4 /dev/sdz1 rw
";

fn measured(blocks: u64, bfree: u64, bavail: u64, files: u64, ffree: u64) -> MountSizes {
    MountSizes::Measured { blocks, bfree, bavail, frsize: 4096, files, ffree }
}

/// The sizes of [`TABLE`].
fn snapshot() -> MountSnapshot {
    let home = measured(480_431_169, 159_457_000, 135_056_794, 122_101_760, 116_904_956);
    let tmpfs = measured(8_114_083, 8_107_233, 8_107_233, 1_048_576, 1_047_592);
    let sizes = BTreeMap::from([
        (21, measured(51_328_915, 25_637_330, 23_012_345, 13_107_200, 12_351_813)),
        (22, measured(8_094_902, 8_094_902, 8_094_902, 8_094_632, 8_093_773)),
        (23, measured(8_114_083, 8_113_650, 8_113_650, 1_048_576, 1_048_559)),
        (24, MountSizes::Dummy),
        (25, MountSizes::Dummy),
        (26, MountSizes::Dummy),
        (27, MountSizes::Dummy),
        (28, tmpfs),
        (29, home),
        (30, home),
        (31, measured(244_190_646, 100_000_000, 100_000_000, 0, 0)),
        (32, MountSizes::Timeout),
        (33, measured(1_623_150, 1_621_900, 1_621_900, 1_622_816, 1_622_696)),
        (34, MountSizes::Masked),
        (35, MountSizes::OtherUser),
        (36, measured(261_628, 240_862, 240_862, 0, 0)),
        (37, MountSizes::Failed { errno: 2 }),
    ]);
    MountSnapshot::new(1_760_000_000, sizes)
}

fn df(args: &str) -> Option<String> {
    let args: Vec<OsString> = args.split_whitespace().map(OsString::from).collect();
    render_df(&parse_mountinfo(TABLE.as_bytes()), &snapshot(), &args)
}

#[test]
fn parse_mountinfo_unescapes_octal() {
    let text = b"36 35 98:0 /a\\134b /mnt/my\\040disk rw,noatime master:1 - ext3 /dev/x\\011y rw,errors=continue\n\
                 37 35 0:3 / /tab rw - tmpfs tm\\012p rw\n\
                 not a line\n\
                 38 35 0:4 / /bad\\9xx rw - tmpfs tmpfs rw\n";
    let entries = parse_mountinfo(text);
    assert_eq!(entries.len(), 3, "the line that does not parse is left out: {entries:?}");
    assert_eq!(
        entries[0],
        MountEntry {
            id: 36,
            parent: 35,
            major: 98,
            minor: 0,
            root: PathBuf::from("/a\\b"),
            target: PathBuf::from("/mnt/my disk"),
            fstype: "ext3".to_owned(),
            source: "/dev/x\ty".to_owned(),
            options: "rw,noatime".to_owned(),
            super_options: "rw,errors=continue".to_owned(),
        }
    );
    assert_eq!(entries[1].source, "tm\np");
    assert_eq!(entries[1].super_options, "rw");
    // A backslash without three octal digits stays.
    assert_eq!(entries[2].target, Path::new("/bad\\9xx"));
}

#[test]
fn parse_mountinfo_keeps_bytes_that_are_not_utf8() {
    let entries = parse_mountinfo(b"40 1 0:9 / /mnt/\\377x rw - tmpfs tmpfs rw\n");
    use std::os::unix::ffi::OsStrExt as _;
    assert_eq!(entries[0].target.as_os_str().as_bytes(), b"/mnt/\xffx");
}

#[test]
fn render_df_matches_gnu_layout() {
    let cases: &[(&str, &str)] = &[
        (
            "",
            "\
Filesystem      1K-blocks       Used Available Use% Mounted on
/dev/nvme1n1p3  205315660  102766340  92049380  53% /
devtmpfs         32379608          0  32379608   0% /dev
tmpfs            32456332       1732  32454600   1% /dev/shm
tmpfs            32456332      27400  32428932   1% /tmp
/dev/nvme0n1p1 1921724676 1283896676 540227176  71% /home
host:/export    976762584  576762584 400000000  60% /mnt/my disk
nas:/vol                -          -         -    - /mnt/nas
tmpfs             6492600       5000   6487600   1% /run/user/1000
gvfsd-fuse              -          -         -    - /run/user/1000/gvfs
tmpfs                   -          -         -    - /run/user/1001
/dev/nvme1n1p1    1046512      83064    963448   8% /boot
",
        ),
        (
            "-h",
            "\
Filesystem      Size  Used Avail Use% Mounted on
/dev/nvme1n1p3  196G   99G   88G  53% /
devtmpfs         31G     0   31G   0% /dev
tmpfs            31G  1.7M   31G   1% /dev/shm
tmpfs            31G   27M   31G   1% /tmp
/dev/nvme0n1p1  1.8T  1.2T  516G  71% /home
host:/export    932G  551G  382G  60% /mnt/my disk
nas:/vol           -     -     -    - /mnt/nas
tmpfs           6.2G  4.9M  6.2G   1% /run/user/1000
gvfsd-fuse         -     -     -    - /run/user/1000/gvfs
tmpfs              -     -     -    - /run/user/1001
/dev/nvme1n1p1 1022M   82M  941M   8% /boot
",
        ),
        (
            "-hT",
            "\
Filesystem     Type             Size  Used Avail Use% Mounted on
/dev/nvme1n1p3 ext4             196G   99G   88G  53% /
devtmpfs       devtmpfs          31G     0   31G   0% /dev
tmpfs          tmpfs             31G  1.7M   31G   1% /dev/shm
tmpfs          tmpfs             31G   27M   31G   1% /tmp
/dev/nvme0n1p1 ext4             1.8T  1.2T  516G  71% /home
host:/export   fuse.sshfs       932G  551G  382G  60% /mnt/my disk
nas:/vol       nfs4                -     -     -    - /mnt/nas
tmpfs          tmpfs            6.2G  4.9M  6.2G   1% /run/user/1000
gvfsd-fuse     fuse.gvfsd-fuse     -     -     -    - /run/user/1000/gvfs
tmpfs          tmpfs               -     -     -    - /run/user/1001
/dev/nvme1n1p1 vfat            1022M   82M  941M   8% /boot
",
        ),
        (
            "-i",
            "\
Filesystem        Inodes   IUsed     IFree IUse% Mounted on
/dev/nvme1n1p3  13107200  755387  12351813    6% /
devtmpfs         8094632     859   8093773    1% /dev
tmpfs            1048576      17   1048559    1% /dev/shm
tmpfs            1048576     984   1047592    1% /tmp
/dev/nvme0n1p1 122101760 5196804 116904956    5% /home
host:/export           0       0         0     - /mnt/my disk
nas:/vol               -       -         -     - /mnt/nas
tmpfs            1622816     120   1622696    1% /run/user/1000
gvfsd-fuse             -       -         -     - /run/user/1000/gvfs
tmpfs                  -       -         -     - /run/user/1001
/dev/nvme1n1p1         0       0         0     - /boot
",
        ),
        (
            "--output=source,target",
            "\
Filesystem     Mounted on
/dev/nvme1n1p3 /
devtmpfs       /dev
tmpfs          /dev/shm
tmpfs          /tmp
/dev/nvme0n1p1 /home
host:/export   /mnt/my disk
nas:/vol       /mnt/nas
tmpfs          /run/user/1000
gvfsd-fuse     /run/user/1000/gvfs
tmpfs          /run/user/1001
/dev/nvme1n1p1 /boot
",
        ),
        (
            "-x tmpfs",
            "\
Filesystem      1K-blocks       Used Available Use% Mounted on
/dev/nvme1n1p3  205315660  102766340  92049380  53% /
devtmpfs         32379608          0  32379608   0% /dev
/dev/nvme0n1p1 1921724676 1283896676 540227176  71% /home
host:/export    976762584  576762584 400000000  60% /mnt/my disk
nas:/vol                -          -         -    - /mnt/nas
gvfsd-fuse              -          -         -    - /run/user/1000/gvfs
/dev/nvme1n1p1    1046512      83064    963448   8% /boot
",
        ),
        (
            "--total",
            "\
Filesystem      1K-blocks       Used  Available Use% Mounted on
/dev/nvme1n1p3  205315660  102766340   92049380  53% /
devtmpfs         32379608          0   32379608   0% /dev
tmpfs            32456332       1732   32454600   1% /dev/shm
tmpfs            32456332      27400   32428932   1% /tmp
/dev/nvme0n1p1 1921724676 1283896676  540227176  71% /home
host:/export    976762584  576762584  400000000  60% /mnt/my disk
nas:/vol                -          -          -    - /mnt/nas
tmpfs             6492600       5000    6487600   1% /run/user/1000
gvfsd-fuse              -          -          -    - /run/user/1000/gvfs
tmpfs                   -          -          -    - /run/user/1001
/dev/nvme1n1p1    1046512      83064     963448   8% /boot
total          3208634304 1963542796 1136990744  64% -
",
        ),
        (
            "-a",
            "\
Filesystem      1K-blocks       Used Available Use% Mounted on
/dev/nvme1n1p3  205315660  102766340  92049380  53% /
devtmpfs         32379608          0  32379608   0% /dev
tmpfs            32456332       1732  32454600   1% /dev/shm
proc                    0          0         0    - /proc
sysfs                   0          0         0    - /sys
cgroup2                 0          0         0    - /sys/fs/cgroup
systemd-1               -          -         -    - /proc/sys/fs/binfmt_misc
tmpfs            32456332      27400  32428932   1% /tmp
/dev/nvme0n1p1 1921724676 1283896676 540227176  71% /home
/dev/nvme0n1p1 1921724676 1283896676 540227176  71% /srv/data
host:/export    976762584  576762584 400000000  60% /mnt/my disk
nas:/vol                -          -         -    - /mnt/nas
tmpfs             6492600       5000   6487600   1% /run/user/1000
gvfsd-fuse              -          -         -    - /run/user/1000/gvfs
tmpfs                   -          -         -    - /run/user/1001
/dev/nvme1n1p1    1046512      83064    963448   8% /boot
/dev/sdz1               -          -         -    - /media/gone
",
        ),
        (
            "-l -h",
            "\
Filesystem      Size  Used Avail Use% Mounted on
/dev/nvme1n1p3  196G   99G   88G  53% /
devtmpfs         31G     0   31G   0% /dev
tmpfs            31G  1.7M   31G   1% /dev/shm
tmpfs            31G   27M   31G   1% /tmp
/dev/nvme0n1p1  1.8T  1.2T  516G  71% /home
tmpfs           6.2G  4.9M  6.2G   1% /run/user/1000
gvfsd-fuse         -     -     -    - /run/user/1000/gvfs
tmpfs              -     -     -    - /run/user/1001
/dev/nvme1n1p1 1022M   82M  941M   8% /boot
",
        ),
    ];
    for (args, table) in cases {
        assert_eq!(df(args).as_deref(), Some(*table), "df {args}");
    }
}

#[test]
fn render_df_reads_options_as_gnu_df_does() {
    // Clusters, a value in the same word or the next, long names and their prefixes.
    assert_eq!(df("-h -T"), df("-hT"));
    assert_eq!(df("--human-readable --print-type"), df("-hT"));
    assert_eq!(df("--hum --print"), df("-hT"));
    assert_eq!(df("-xtmpfs"), df("-x tmpfs"));
    assert_eq!(df("--exclude-type=tmpfs"), df("-x tmpfs"));
    assert_eq!(df("--exclude-type tmpfs"), df("-x tmpfs"));
    assert_eq!(df("--sync -k"), df(""));
    assert_eq!(df("-B 1K"), df(""));
    // The last unit wins.
    assert_eq!(df("-h -k"), df(""));
    let header = |args: &str| df(args).unwrap().lines().next().unwrap().to_owned();
    assert_eq!(header("-B1M"), "Filesystem     1M-blocks    Used Available Use% Mounted on");
    assert_eq!(header("-BM"), "Filesystem     1M-blocks     Used Available Use% Mounted on");
    assert_eq!(header("-B 1000"), "Filesystem     1kB-blocks       Used Available Use% Mounted on");
    assert_eq!(header("-P"), "Filesystem     1024-blocks       Used Available Capacity Mounted on");
    let root = |args: &str| df(args).unwrap().lines().nth(1).unwrap().to_owned();
    assert_eq!(root("-BM"), "/dev/nvme1n1p3   200504M  100358M    89892M  53% /");
    assert_eq!(root("-H"), "/dev/nvme1n1p3  211G  106G   95G  53% /");
    assert_eq!(root("-t ext4 -h"), "/dev/nvme1n1p3  196G   99G   88G  53% /");
}

#[test]
fn render_df_unsupported_option_exits_2() {
    for args in [
        "--help",
        "--version",
        "--he",
        "--h",
        "--t",
        "/home",
        "-h /home",
        "--",
        "-",
        "-Z",
        "--direct",
        "--output -i",
        "-i --output",
        "-T --output=source",
        "--output=source,source",
        "--output=nothing",
        "-t tmpfs -x tmpfs",
        "-B",
        "-B 0",
        "-B 1X",
        "-B 'K",
        "--all=yes",
        "-t nothing",
    ] {
        assert_eq!(df(args), None, "df {args}");
    }
}

#[test]
fn render_findmnt_df_layout() {
    let entries = parse_mountinfo(TABLE.as_bytes());
    assert_eq!(
        render_findmnt_df(&entries, &snapshot(), false),
        "\
SOURCE                   FSTYPE       SIZE  USED  AVAIL USE% TARGET
/dev/nvme1n1p3           ext4       195.8G   98G  87.8G  50% /
devtmpfs                 devtmpfs    30.9G     0  30.9G   0% /dev
tmpfs                    tmpfs         31G  1.7M    31G   0% /dev/shm
tmpfs                    tmpfs         31G 26.8M  30.9G   0% /tmp
/dev/nvme0n1p1           ext4         1.8T  1.2T 515.2G  67% /home
/dev/nvme0n1p1[/me/data] ext4         1.8T  1.2T 515.2G  67% /srv/data
host:/export             fuse.sshfs 931.5G  550G 381.5G  59% /mnt/my disk
nas:/vol                 nfs4            -     -      -    - /mnt/nas
tmpfs                    tmpfs        6.2G  4.9M   6.2G   0% /run/user/1000
tmpfs                    tmpfs           -     -      -    - /run/user/1001
/dev/nvme1n1p1           vfat        1022M 81.1M 940.9M   8% /boot
/dev/sdz1                ext4            -     -      -    - /media/gone
"
    );
    assert_eq!(
        render_findmnt_df(&entries, &snapshot(), true),
        "\
SOURCE                   FSTYPE     INO.TOTAL INO.USED INO.AVAIL INO.USE% TARGET
/dev/nvme1n1p3           ext4        13107200   755387  12351813       6% /
devtmpfs                 devtmpfs     8094632      859   8093773       0% /dev
tmpfs                    tmpfs        1048576       17   1048559       0% /dev/shm
tmpfs                    tmpfs        1048576      984   1047592       0% /tmp
/dev/nvme0n1p1           ext4       122101760  5196804 116904956       4% /home
/dev/nvme0n1p1[/me/data] ext4       122101760  5196804 116904956       4% /srv/data
host:/export             fuse.sshfs         0        0         0        - /mnt/my disk
nas:/vol                 nfs4               -        -         -        - /mnt/nas
tmpfs                    tmpfs        1622816      120   1622696       0% /run/user/1000
tmpfs                    tmpfs              -        -         -        - /run/user/1001
/dev/nvme1n1p1           vfat               0        0         0        - /boot
/dev/sdz1                ext4               -        -         -        - /media/gone
"
    );
}

#[test]
fn dummy_types_are_never_measured() {
    for fstype in ["autofs", "proc", "sysfs", "cgroup2", "nsfs", "fuse.portal", "binfmt_misc"] {
        assert!(is_dummy(fstype), "{fstype}");
    }
    for fstype in ["ext4", "tmpfs", "fuse.sshfs", "nfs4", "vfat", "efivarfs", "overlay"] {
        assert!(!is_dummy(fstype), "{fstype}");
    }
}

#[test]
fn other_users_mounts_are_their_runtime_dirs_and_fuse_servers() {
    let entries = parse_mountinfo(TABLE.as_bytes());
    let others: Vec<&str> = entries
        .iter()
        .filter(|entry| is_other_users(entry, 1000))
        .map(|entry| entry.target.to_str().unwrap())
        .collect();
    assert_eq!(others, ["/run/user/1001"]);
    let others: Vec<&str> = entries
        .iter()
        .filter(|entry| is_other_users(entry, 1001))
        .map(|entry| entry.target.to_str().unwrap())
        .collect();
    assert_eq!(others, ["/mnt/my disk", "/run/user/1000", "/run/user/1000/gvfs"]);
}

#[test]
fn a_mount_below_a_mask_is_masked_and_the_runtime_dir_itself_is_not() {
    let masks = [PathBuf::from("/home/me/.vault")];
    let below = [PathBuf::from("/run/user/1000")];
    let masked = |target: &str| is_masked(Path::new(target), &masks, &below);
    assert!(masked("/home/me/.vault"));
    assert!(masked("/home/me/.vault/inner"));
    assert!(!masked("/home/me/.vault2"));
    assert!(masked("/run/user/1000/gvfs"));
    assert!(masked("/run/user/1000/doc"));
    assert!(!masked("/run/user/1000"));
    assert!(!masked("/run/user/10001"));
    assert!(!masked("/home"));
}

#[test]
fn the_snapshot_reads_back_and_a_statvfs_line_gives_its_sizes() {
    let snapshot = snapshot();
    assert_eq!(MountSnapshot::from_json(&snapshot.to_json().unwrap()).unwrap(), snapshot);
    let json = String::from_utf8(
        MountSnapshot::new(1, BTreeMap::from([(7, MountSizes::Timeout)])).to_json().unwrap(),
    )
    .unwrap();
    assert_eq!(json, r#"{"taken_at":1,"sizes":{"7":{"kind":"timeout"}}}"#);
    let too_large = vec![b' '; MAX_MOUNTS_BYTES + 1];
    assert!(MountSnapshot::from_json(&too_large).is_err());

    let ok: StatvfsLine = serde_json::from_str(
        r#"{"path":"/","ok":{"blocks":1,"bfree":2,"bavail":3,"frsize":4,"files":5,"ffree":6}}"#,
    )
    .unwrap();
    assert_eq!(
        ok.sizes(),
        MountSizes::Measured { blocks: 1, bfree: 2, bavail: 3, frsize: 4, files: 5, ffree: 6 }
    );
    let failed: StatvfsLine = serde_json::from_str(r#"{"path":"/x","errno":107}"#).unwrap();
    assert_eq!(failed.sizes(), MountSizes::Failed { errno: 107 });
    assert_eq!(serde_json::to_string(&failed).unwrap(), r#"{"path":"/x","errno":107}"#);
}

#[test]
fn render_df_counts_the_device_of_stat_as_gnu_df_does() {
    // Two btrfs subvolumes share the device of their mountinfo lines; stat gives each
    // one a device of its own.
    let table = "\
1 0 0:34 /@ / rw - btrfs /dev/sda2 rw,subvol=/@
2 1 0:34 /@home /home rw - btrfs /dev/sda2 rw,subvol=/@home
";
    let sizes =
        BTreeMap::from([(1, measured(1000, 500, 400, 0, 0)), (2, measured(1000, 500, 400, 0, 0))]);
    let entries = parse_mountinfo(table.as_bytes());
    let targets = |snapshot: &MountSnapshot| -> Vec<String> {
        let args = [OsString::from("--output=target")];
        let text = render_df(&entries, snapshot, &args).unwrap();
        text.lines().skip(1).map(|line| line.trim_end().to_owned()).collect()
    };
    let plain = MountSnapshot::new(1, sizes.clone());
    assert_eq!(targets(&plain), ["/"], "one device: the target nearer the root");
    let devices = BTreeMap::from([(1, (0, 35)), (2, (0, 36))]);
    let with_stat = MountSnapshot::new(1, sizes).with_devices(devices);
    assert_eq!(targets(&with_stat), ["/", "/home"]);
    assert_eq!(with_stat.device(&entries[1]), (0, 36));
    let json = String::from_utf8(with_stat.to_json().unwrap()).unwrap();
    assert!(json.contains(r#""devices":{"1":[0,35],"2":[0,36]}"#), "{json}");
    assert_eq!(MountSnapshot::from_json(json.as_bytes()).unwrap(), with_stat);
}
