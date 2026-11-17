fn main() {
    let d = spike_data::Dataset::from_env();
    let groups = d.rects.iter().filter(|r| r.group).count();
    let labels = d.rects.iter().filter(|r| r.label).count();
    println!(
        "nodes {} · rows {} · rects {} ({} groups, {} labelled) · root {} · {} files · built in {:.0} ms",
        d.tree.len(),
        d.rows.len(),
        d.rects.len(),
        groups,
        labels,
        spike_data::fmt::size(d.tree.size[0]),
        spike_data::fmt::count(d.tree.files[0]),
        d.build_ms
    );
}
