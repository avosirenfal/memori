//! The report as JSON: one key per chosen section, named after its flag, in report order.
//! All sizes are numbers of bytes. A section that doesn't apply to this system is `null`;
//! one that couldn't be read is `{"error": "..."}`.

use serde::Serialize;
use serde::ser::SerializeStruct;
use serde_json::{Map, Value, to_value};

use super::View;
use crate::analysis::breakdown::{Reclaim, Slice};
use crate::analysis::{Group, Overview, Report};
use crate::sections::SectionKind;
use crate::units::Bytes;

pub fn json(report: &Report, view: &View) -> serde_json::Result<String> {
    // The host is the JSON counterpart of the title line, so it follows `--headers`.
    let host = view.show.headers.then(|| Ok(("host".to_owned(), to_value(&report.host)?)));
    let sections = view
        .sections
        .iter()
        .map(|&kind| Ok((kind.spec().flag.to_owned(), section(kind, report, view.breakdown)?)));
    let map: Map<String, Value> =
        host.into_iter().chain(sections).collect::<serde_json::Result<_>>()?;
    Ok(format!("{}\n", serde_json::to_string_pretty(&map)?))
}

fn section(kind: SectionKind, report: &Report, breakdown: bool) -> serde_json::Result<Value> {
    match kind {
        SectionKind::Overview => to_value(OverviewJson { overview: &report.overview, breakdown }),
        SectionKind::Programs => to_value(&report.programs),
        SectionKind::Zfs => to_value(&report.zfs),
        SectionKind::Kernel => to_value(&report.kernel),
        SectionKind::Cache => to_value(&report.cache),
        SectionKind::Swap => to_value(&report.swap),
        SectionKind::Fragmentation => to_value(&report.fragmentation),
        SectionKind::Unexplained => to_value(&report.unexplained),
    }
}

/// The overview, with each group's categories and the classic "used" figure only when
/// the breakdown is asked for (`--detailed`), as in the text report.
struct OverviewJson<'a> {
    overview: &'a Overview,
    breakdown: bool,
}

#[derive(Serialize)]
struct GroupJson {
    reclaim: Reclaim,
    bytes: Bytes,
    #[serde(skip_serializing_if = "Option::is_none")]
    categories: Option<Vec<Slice>>,
}

impl Serialize for OverviewJson<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let o = self.overview;
        let groups: Vec<GroupJson> = o
            .groups
            .iter()
            .map(|group: &Group| GroupJson {
                reclaim: group.reclaim,
                bytes: group.bytes(),
                categories: self.breakdown.then(|| group.slices.iter().copied().collect()),
            })
            .collect();
        let mut out = serializer.serialize_struct("Overview", 9)?;
        out.serialize_field("total", &o.total)?;
        out.serialize_field("groups", &groups)?;
        out.serialize_field("overcounted", &o.overcounted)?;
        out.serialize_field("adjustment", &o.adjustment)?;
        out.serialize_field("available", &o.available)?;
        out.serialize_field("arc_shrink", &o.arc_shrink)?;
        out.serialize_field("total_available", &o.total_available())?;
        if self.breakdown {
            out.serialize_field("classic_used", &o.classic_used)?;
        }
        out.end()
    }
}
