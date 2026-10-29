//! End-to-end tests: hand-written kernel files → snapshot → analysis → rendered text.

use super::*;
use crate::analysis::breakdown::Breakdown;
use crate::analysis::{Grouping, Limit, Settings, analyze};
use crate::snapshot::Snapshot;
use crate::sources::procs::{Process, ProcessScan, Rollup};
use crate::sources::slab::Reclaimability;
use crate::sources::swap::Zram;
use crate::sources::{buddy, meminfo, slab, swap, zfs};

const GIB: u64 = 1 << 30;

/// A 64 GiB ZFS workstation with zram swap.
fn zfs_workstation() -> Snapshot {
    let meminfo = meminfo::parse(
        "MemTotal: 65536000 kB\nMemFree: 4000000 kB\nMemAvailable: 12000000 kB\n\
         Buffers: 100000 kB\nCached: 9000000 kB\nSwapCached: 50000 kB\n\
         Active(file): 5000000 kB\nInactive(file): 2000000 kB\nSwapTotal: 8388604 kB\n\
         SwapFree: 6000000 kB\nDirty: 3000 kB\nAnonPages: 20000000 kB\nMapped: 900000 kB\n\
         Shmem: 2000000 kB\nSReclaimable: 1500000 kB\nSUnreclaim: 4000000 kB\n\
         KernelStack: 40000 kB\nPageTables: 300000 kB\nVmallocUsed: 600000 kB\n\
         Percpu: 20000 kB\nHugetlb: 0 kB\n",
    )
    .unwrap();
    let arc = zfs::parse(&format!(
        "header\nname type data\nhits 4 9800\nmisses 4 200\nsize 4 {}\nc 4 {}\n\
         c_min 4 {}\nc_max 4 {}\ndata_size 4 {}\nmetadata_size 4 {}\nhdr_size 4 {}\n\
         dbuf_size 4 {}\ndnode_size 4 {}\nbonus_size 4 0\n",
        22 * GIB,
        22 * GIB,
        2 * GIB,
        32 * GIB,
        17 * GIB,
        3 * GIB,
        GIB / 2,
        GIB / 2,
        GIB,
    ))
    .unwrap();
    let slabs = slab::parse(
        "slabinfo - version: 2.1\n\
         dnode_t 900000 1000000 896 36 8 : tunables 0 0 0 : slabdata 27000 27778 0\n\
         kmalloc-64 9000000 16000000 64 64 1 : tunables 0 0 0 : slabdata 250000 250000 0\n\
         dentry 3000000 3100000 192 21 1 : tunables 0 0 0 : slabdata 147000 147619 0\n",
        Bytes(4096),
        |name| match name {
            "dentry" => Reclaimability::Reclaimable,
            _ => Reclaimability::Unreclaimable,
        },
    )
    .unwrap();
    let process = |pid: u32, parent: u32, name: &str, gib: u64| Process {
        pid,
        parent,
        started: u64::from(pid),
        name: name.to_owned(),
        command: Some(format!("/usr/bin/{name} --flag")),
        mem: Rollup { pss: Bytes(gib * GIB), swap: Bytes(gib * GIB / 20) },
    };
    Snapshot {
        hostname: Some("workstation".to_owned()),
        kernel: Some("6.12.0".to_owned()),
        page_size: Bytes(4096),
        meminfo,
        arc: Source::Available(arc),
        slabs: Source::Available(slabs),
        processes: ProcessScan {
            processes: vec![
                process(100, 1, "firefox", 6),
                process(101, 100, "firefox", 3),
                process(2909719, 1, "java", 5),
                process(300, 1, "Xorg", 1),
            ],
            unreadable: 3,
        },
        buddy: Source::Available(
            buddy::parse("Node 0, zone Normal 90000 40000 9000 2000 500 100 20 4 1 0 0\n").unwrap(),
        ),
        swaps: Source::Available(
            swap::parse_swaps(
                "Filename Type Size Used Priority\n/dev/zram0 partition 8388608 2097152 100\n",
            )
            .unwrap(),
        ),
        zram: vec![Zram {
            name: "zram0".to_owned(),
            original: Bytes(2 * GIB),
            ram_used: Bytes(GIB / 2),
        }],
        zsmalloc_pool: Bytes(GIB / 2),
        tmpfs: vec![],
        gpus: vec![],
    }
}

const SETTINGS: Settings =
    Settings { programs: Limit::Top(2), grouping: Grouping::ByProgram, slabs: Limit::Top(5) };
const SHOW_ALL: Show = Show { headers: true, help: true };

fn rendered(settings: &Settings, view: &View) -> String {
    let snapshot = zfs_workstation();
    let text = strip_ansi(&render(&analyze(&snapshot, settings), view).unwrap());
    println!("{text}");
    text
}

#[test]
fn summary_shows_sums_and_programs_only() {
    let view = View {
        sections: SectionKind::SUMMARY.to_vec(),
        breakdown: false,
        show: SHOW_ALL,
        format: Format::Text,
    };
    let text = rendered(&SETTINGS, &view);
    assert!(text.contains("Total available"));
    assert!(text.contains("31.4 GiB")); // 11.4 GiB MemAvailable + 20.0 GiB ARC shrink
    assert!(text.contains("Held back by the kernel"));
    assert!(text.contains("2909719  java"));
    assert!(text.contains("100  firefox"));
    assert!(text.contains("…1 more"));
    assert!(!text.contains("Page tables"));
    assert!(!text.contains("KERNEL"));
}

#[test]
fn detailed_report_covers_every_source() {
    let view = View {
        sections: SectionKind::ALL.to_vec(),
        breakdown: true,
        show: SHOW_ALL,
        format: Format::Text,
    };
    let text = rendered(&SETTINGS, &view);
    for kind in SectionKind::ALL {
        assert!(text.contains(kind.spec().title), "missing {}", kind.spec().title);
    }
    assert!(text.contains("Page tables"));
    assert!(text.contains("4.0× compressed"));
    assert!(text.contains("/dev/zram0 (zram)"));
    assert!(text.contains("fragmented: huge pages need compaction"));
}

#[test]
fn bare_program_list_has_no_headers_or_help() {
    let settings = Settings { programs: Limit::All, ..SETTINGS };
    let view = View {
        sections: vec![SectionKind::Programs],
        breakdown: false,
        show: Show { headers: false, help: false },
        format: Format::Text,
    };
    let text = rendered(&settings, &view);
    assert_eq!(text.lines().count(), 3);
    assert!(text.contains("Xorg"));
    assert!(!text.contains("PID"));
    assert!(!text.contains("processes"));
    assert!(!text.contains("unreadable"));
}

#[test]
fn arc_bookkeeping_is_counted_as_slab_not_twice() {
    let snapshot = zfs_workstation();
    let breakdown = Breakdown::of(&snapshot);
    assert_eq!(breakdown.bytes(Category::ZfsArc), Bytes(20 << 30));
    assert_eq!(breakdown.bytes(Category::CompressedSwap), Bytes(512 << 20));
}

#[test]
fn cells_add_up_to_width() {
    assert_eq!(allocate_cells(&[1, 1, 1], 10), vec![4, 3, 3]);
    assert_eq!(allocate_cells(&[0, 5], 4), vec![0, 4]);
    assert_eq!(allocate_cells(&[0, 0], 4), vec![0, 0]);
    assert_eq!(allocate_cells(&[7, 13, 29, 1, 50], 72).iter().sum::<usize>(), 72);
}

#[test]
fn readme_sample_is_current() {
    let show = Show { headers: true, help: false };
    let view = View {
        sections: SectionKind::SUMMARY.to_vec(),
        breakdown: false,
        show,
        format: Format::Text,
    };
    let text = rendered(&SETTINGS, &view);
    // The README leaves out the title line, which names the test machine.
    let sample = text.split_once("\n\n").map_or(text.as_str(), |(_, rest)| rest).trim_end();
    assert!(
        include_str!("../../README.md").contains(sample),
        "README.md's sample is out of date; replace it with:\n\n{sample}"
    );
}

fn json(settings: &Settings, view: &View) -> serde_json::Value {
    let snapshot = zfs_workstation();
    let text = render(&analyze(&snapshot, settings), view).unwrap();
    println!("{text}");
    serde_json::from_str(&text).unwrap()
}

fn json_view(sections: &[SectionKind], breakdown: bool, headers: bool) -> View {
    View {
        sections: sections.to_vec(),
        breakdown,
        show: Show { headers, help: true },
        format: Format::Json,
    }
}

#[test]
fn json_has_one_key_per_chosen_section_in_order() {
    let value = json(&SETTINGS, &json_view(&SectionKind::ALL, true, true));
    let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    let expected: Vec<&str> =
        std::iter::once("host").chain(SectionKind::ALL.map(|kind| kind.spec().flag)).collect();
    assert_eq!(keys, expected);
    assert_eq!(value["zfs"]["sizes"]["size"], 22 * GIB);
    assert_eq!(value["swap"]["devices"][0]["kind"], "zram");
    assert_eq!(value["fragmentation"]["verdict"], "fragmented");
    assert_eq!(value["fragmentation"]["free"]["large"], 0);
    assert_eq!(value["kernel"]["slabs"]["largest"][2]["reclaimability"], "reclaimable");
    assert_eq!(value["overview"]["total_available"], 12_000_000 * 1024 + 20 * GIB);
    assert_eq!(value["overview"]["groups"][1]["categories"][0]["category"], "zfs_arc");
}

#[test]
fn json_respects_settings() {
    let settings = Settings { grouping: Grouping::PerProcess, ..SETTINGS };
    let value = json(&settings, &json_view(&[SectionKind::Programs], false, false));
    let object = value.as_object().unwrap();
    assert_eq!(object.keys().collect::<Vec<_>>(), ["programs"]);
    let rows = value["programs"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["kind"], "process");
    assert_eq!(rows[0]["command"], "/usr/bin/firefox --flag");
    assert_eq!(value["programs"]["hidden"]["rows"], 2);

    let grouped = json(&SETTINGS, &json_view(&[SectionKind::Programs], false, true));
    assert_eq!(grouped["programs"]["rows"][0]["kind"], "program");
    assert_eq!(grouped["programs"]["rows"][0]["processes"], 2);
    assert_eq!(grouped["programs"]["rows"][0]["pid"], 100);
}

#[test]
fn json_overview_breaks_down_only_when_detailed() {
    let summary = json(&SETTINGS, &json_view(&[SectionKind::Overview], false, true));
    assert!(summary["overview"]["groups"][0].get("categories").is_none());
    assert!(summary["overview"].get("classic_used").is_none());
    let detailed = json(&SETTINGS, &json_view(&[SectionKind::Overview], true, true));
    assert!(detailed["overview"]["groups"][0]["categories"].is_array());
    assert!(detailed["overview"]["classic_used"].is_number());
}
