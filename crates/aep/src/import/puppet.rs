//! After Effects' Puppet effect (`ADBE FreePin3`) arrives as EffectCraft's Puppet
//! (`ec.distort.puppet`) in its place in the stack, with its name and fx switch, its meshes and
//! their Deform pins.
//!
//! **A mesh's data** (`fpms`, after the `ADBE FreePin3 Mesh` match name, beside `fpmv` and
//! `fpsz`) holds the auto-traced outline, then the mesh: a big-endian `u32` vertex count `V`,
//! `V` vertices as pairs of big-endian `f64` (x, y) in **fractions of the layer width** (both
//! coordinates: on a 2560x1440 layer the mesh's y runs to 0.5625), a `u32`
//! triangle count `T` and `T` triples of `u32` vertex indices, which end the data (one pad byte
//! may follow). [`mesh_vertices`] finds the vertex count by that structure, so the outline in
//! front of it needs no decoding, and a block that does not add up is refused.
//!
//! **A pin's rest place** is the mesh vertex its `Vtx Index` names (After Effects inserts a
//! vertex for every pin). Its Position (keyed or static) is stored in fractions of the layer size
//! (x of the width, y of the height), Scale in percent and Rotation in degrees. A pin with Scale or Rotation is an Advanced pin.
//! Byte sample: a Deform mesh of five keyed pins on a 2560x1440 layer saved by After Effects 24.6,
//! whose `Vtx Index` values 8..12 name the vertices that equal the pins' first Position keys to
//! the pixel. Whether a layer taller than wide scales the mesh by its width or by its height is
//! not sampled: such a pin is imported scaled by the width and reported.
//!
//! Still reported, never guessed: Overlap and Stiffness (Starch) pins, a non-zero `Vtx Offset`,
//! pin expressions, a mesh whose data does not add up. A Puppet whose `arap` group holds no mesh
//! group has no pins (the group comes with the first pin) and loses nothing.

use effectcraft_project::build::Ids;
use effectcraft_project::{Node, PropGroup};

use effectcraft_effects::puppet::{MeshOpts, PinKind, mesh_group, pin_group};
use effectcraft_keyframe::Value;

use crate::import::effects::{LayerCtx, carries_nothing};
use crate::import::props::{Kind, import_keys};
use crate::import::report::{Collector, LossCategory};
use crate::{Effect as AepEffect, EffectDefinitions, ParamValue, PropGroup as AepGroup, PropNode, Property as AepProperty};

/// After Effects' Puppet effect.
pub(crate) const MATCH_NAME: &str = "ADBE FreePin3";
/// The hidden group that holds the meshes (`arap`).
const ARAP: &str = "ADBE FreePin3 ARAP Group";
/// Auto-traced Shapes, beside the meshes.
const OUTLINES: &str = "ADBE FreePin3 Outlines";
/// The hidden group of the meshes.
const MESH_GROUP: &str = "ADBE FreePin3 Mesh Group";
/// One mesh (`Mesh 1`).
const MESH_ATOM: &str = "ADBE FreePin3 Mesh Atom";
/// A mesh's hidden triangulation.
const MESH_DATA: &str = "ADBE FreePin3 Mesh";
const TRIANGLES: &str = "ADBE FreePin3 Mesh Tri Count";
const EXPANSION: &str = "ADBE FreePin3 Mesh Expansion";
/// The Deform pin container and the parts of one Deform pin.
const POS_PINS: &str = "ADBE FreePin3 PosPins";
const POS_ATOM: &str = "ADBE FreePin3 PosPin Atom";
const VTX_INDEX: &str = "ADBE FreePin3 PosPin Vtx Index";
const VTX_OFFSET: &str = "ADBE FreePin3 PosPin Vtx Offset";
const PIN_POSITION: &str = "ADBE FreePin3 PosPin Position";
const PIN_SCALE: &str = "ADBE FreePin3 PosPin Scale";
const PIN_ROTATION: &str = "ADBE FreePin3 PosPin Rotation";
/// More vertices than this in one mesh is not a Puppet mesh (After Effects caps Triangles at 10 000).
const MAX_VERTICES: usize = 1 << 20;
/// At most this many meshes, pin atom names or unrecognised names are listed in a report line;
/// the rest are counted (`and 3 more`).
const LIST_CAP: usize = 10;
/// The pin containers of a mesh: match name and the name After Effects shows.
const PIN_GROUPS: [(&str, &str); 3] = [("ADBE FreePin3 PosPins", "Deform"), ("ADBE FreePin3 HghtPins", "Overlap"), ("ADBE FreePin3 StarchPins", "Stiffness")];

/// Imports a Puppet instance (the caller checked the match name) into `target`.
pub(crate) fn import_puppet<'f, 'a>(
    defs: &EffectDefinitions<'f, 'a>,
    fx: &AepEffect<'f, 'a>,
    cx: &LayerCtx<'_>,
    ids: &mut Ids<'_>,
    target: &mut PropGroup,
    col: &mut Collector,
) {
    let shown = fx.display_name().unwrap_or_else(|| fx.match_name().to_string());
    let here = format!("{}: '{}' effect '{shown}' ({MATCH_NAME})", cx.comp, cx.layer);
    let Some(spec) = effectcraft_effects::find(effectcraft_effects::puppet::ID) else {
        col.lose(LossCategory::Effects, format!("{}: '{}' has the effect '{shown}' ({MATCH_NAME})", cx.comp, cx.layer));
        return;
    };
    let size = [f64::from(cx.size.0), f64::from(cx.size.1)];
    let mut group = effectcraft_effects::instantiate(spec, ids, &shown, size);
    match fx.enabled() {
        Some(on) => group.enabled = on,
        None => col.lose(LossCategory::DamagedData, format!("{}: '{}': the fx switch of '{shown}' is unreadable, imported switched on", cx.comp, cx.layer)),
    }
    // The ordinary parameters (`parT`): none is mapped yet, so a stored one is reported.
    let params = defs.params(fx);
    for p in &params {
        if carries_nothing(p) {
            continue;
        }
        if p.stored().is_some() && !matches!(p.stored_value(), Some(ParamValue::Layer(0))) {
            let label = if p.name.is_empty() { p.match_name.clone() } else { format!("{} ({})", p.name, p.match_name) };
            col.lose(LossCategory::Effects, format!("{here}: parameter {label} not imported"));
        }
    }
    import_meshes(fx, &params.iter().map(|p| p.match_name.as_str()).collect::<Vec<_>>(), &here, cx, ids, &mut group, col);
    // The effect itself arrives (an EffectCraft Puppet the user can pin again), so it counts as
    // imported; what it held is in the losses above.
    col.imported.effects = col.imported.effects.saturating_add(1);
    target.children.push(Node::Group(group));
}

/// Imports the meshes and pins of `fx` into `group` (the EffectCraft Puppet) and reports what is
/// not imported: every entry of its value group that is neither a `parT` parameter
/// (`param_names`) nor the mesh tree, and whatever a mesh holds that [`import_mesh`] cannot take.
fn import_meshes(fx: &AepEffect<'_, '_>, param_names: &[&str], here: &str, cx: &LayerCtx<'_>, ids: &mut Ids<'_>, group: &mut PropGroup, col: &mut Collector) {
    let mut unknown: Vec<String> = Vec::new();
    let values = fx.chunk().child_list(b"tdgp").and_then(AepGroup::from_chunk);
    let mut arap = None;
    for e in values.map(|g| g.entries()).unwrap_or_default() {
        if e.match_name == ARAP {
            if let PropNode::Group(g) = e.node {
                if arap.is_none() {
                    arap = Some(g);
                }
            } else {
                push_unique(&mut unknown, e.match_name);
            }
        } else if !param_names.contains(&e.match_name.as_str()) {
            match e.node {
                // A value outside `parT` (On Transparent, ...): reported with what it holds.
                PropNode::Property(p) => col.lose(LossCategory::Effects, format!("{here}: property {} ({}) not imported", e.match_name, describe(&p))),
                PropNode::Group(_) | PropNode::Other(_) => push_unique(&mut unknown, e.match_name),
            }
        }
    }
    // `None`: no mesh group entry (no pin was ever placed); `Some(false)`: an entry that is not a
    // group; `Some(true)`: the mesh group, read.
    let mut mesh_group_entry: Option<bool> = None;
    if let Some(arap) = arap {
        for e in arap.entries() {
            match (e.match_name.as_str(), e.node) {
                (MESH_GROUP, PropNode::Group(g)) if mesh_group_entry != Some(true) => {
                    mesh_group_entry = Some(true);
                    for m in g.entries() {
                        match (m.match_name.as_str(), m.node) {
                            (MESH_ATOM, PropNode::Group(mesh)) => {
                                let n = effectcraft_effects::puppet::meshes(group).count().saturating_add(1);
                                if let Err(why) = import_mesh(&mesh, n, here, cx, ids, group, col) {
                                    let summary = mesh_summary(&mesh, n, &mut unknown);
                                    col.lose(LossCategory::Effects, format!("{here}: imported without {summary} ({why})"));
                                }
                            }
                            _ => push_unique(&mut unknown, m.match_name.clone()),
                        }
                    }
                }
                (MESH_GROUP, _) => {
                    if mesh_group_entry.is_none() {
                        mesh_group_entry = Some(false);
                    }
                }
                // Auto-traced Shapes are made from the layer again (EffectCraft traces the alpha).
                (OUTLINES, _) => {}
                _ => push_unique(&mut unknown, e.match_name.clone()),
            }
        }
    }
    let extra = if unknown.is_empty() { String::new() } else { format!("; not recognised: {}", capped(&unknown, ", ")) };
    if arap.is_none() || mesh_group_entry == Some(false) {
        col.lose(
            LossCategory::Effects,
            format!("{here}: imported without meshes or pins: its mesh group ({ARAP} / {MESH_GROUP}) was not found or is not a group{extra}"),
        );
    } else if !unknown.is_empty() {
        col.lose(LossCategory::Effects, format!("{here}: not imported{extra}"));
    }
}

/// Imports one After Effects mesh with its Deform pins into `group`. `Err` says why the mesh is
/// not imported; pins and settings it cannot take are reported, and the rest arrives.
fn import_mesh(
    mesh: &AepGroup<'_, '_>,
    n: usize,
    here: &str,
    cx: &LayerCtx<'_>,
    ids: &mut Ids<'_>,
    group: &mut PropGroup,
    col: &mut Collector,
) -> Result<(), String> {
    let (w, h) = (f64::from(cx.size.0), f64::from(cx.size.1));
    if w <= 0.0 || h <= 0.0 {
        return Err("the layer has no size".into());
    }
    let data = mesh.chunk().child(b"fpms").and_then(|c| c.bytes()).ok_or("its mesh data is missing")?;
    let vertices = mesh_vertices(data).ok_or("its mesh data could not be read")?;
    let name = mesh.user_name().unwrap_or_else(|| format!("Mesh {n}"));
    let mut opts = MeshOpts::default();
    let mut pins: Vec<PropGroup> = Vec::new();
    for e in mesh.entries() {
        match (e.match_name.as_str(), e.node) {
            (TRIANGLES, PropNode::Property(p)) => {
                if let Some(t) = static_number(&p).filter(|t| (1.0..=10_000.0).contains(t)) {
                    opts.triangles = t;
                }
            }
            (EXPANSION, PropNode::Property(p)) => {
                if let Some(x) = static_number(&p).filter(|x| x.abs() <= 10_000.0) {
                    opts.expansion = x;
                }
            }
            (POS_PINS, PropNode::Group(container)) => {
                for pin in container.entries() {
                    match (pin.match_name.as_str(), pin.node) {
                        (POS_ATOM, PropNode::Group(g)) => {
                            let label = g.user_name().unwrap_or_else(|| format!("Puppet Pin {}", pins.len().saturating_add(1)));
                            match import_pin(&g, &label, &vertices, (w, h), here, cx, ids, col) {
                                Ok(p) => pins.push(p),
                                Err(why) => col.lose(LossCategory::Effects, format!("{here}: {name}: pin '{label}' not imported ({why})")),
                            }
                        }
                        (other, _) => col.lose(LossCategory::Effects, format!("{here}: {name}: Deform entry {other} not imported")),
                    }
                }
            }
            (other, PropNode::Group(g)) if PIN_GROUPS.iter().any(|(m, _)| *m == other) => {
                let mut ignored = Vec::new();
                let summary = pins_summary(&g, &mut ignored);
                if summary != "no pins" {
                    let shown = PIN_GROUPS.iter().find(|(m, _)| *m == other).map_or(other, |(_, s)| *s);
                    col.lose(LossCategory::Effects, format!("{here}: {name}: {shown} {summary} not imported"));
                }
            }
            // The triangulation After Effects keeps; EffectCraft builds its own from the alpha.
            (MESH_DATA, _) => {}
            (other, _) => col.lose(LossCategory::Effects, format!("{here}: {name}: entry {other} not imported")),
        }
    }
    if pins.is_empty() {
        // A mesh without pins does not deform the layer.
        return Ok(());
    }
    // EffectCraft traces the mesh from the alpha under the seed, and a pin sits on the mesh.
    let seed = pins.first().and_then(|p| p.get("rest")).map_or([w / 2.0, h / 2.0], |r| r.value.as_vec2());
    let mut m = mesh_group(ids, &name, seed, &opts);
    if let Some(deform) = m.sub_mut("deform") {
        deform.children.extend(pins.into_iter().map(Node::Group));
    }
    group.children.push(Node::Group(m));
    Ok(())
}

/// One Deform pin: its rest place from the mesh vertex it names, then its Position, Scale and
/// Rotation (keyed or static).
#[allow(clippy::too_many_arguments)]
fn import_pin(
    pin: &AepGroup<'_, '_>,
    label: &str,
    vertices: &[[f64; 2]],
    (w, h): (f64, f64),
    here: &str,
    cx: &LayerCtx<'_>,
    ids: &mut Ids<'_>,
    col: &mut Collector,
) -> Result<PropGroup, String> {
    let prop = |m: &str| match pin.get(m) {
        Some(PropNode::Property(p)) => Some(p),
        _ => None,
    };
    let index = prop(VTX_INDEX).and_then(|p| static_number(&p)).ok_or("its mesh vertex is unreadable")?;
    if !(index >= 0.0 && index.fract() == 0.0) {
        return Err(format!("mesh vertex {index} is not a vertex number"));
    }
    let vertex = vertices.get(index as usize).ok_or_else(|| format!("mesh vertex {index} is not in the mesh ({} vertices)", vertices.len()))?;
    if prop(VTX_OFFSET).and_then(|p| static_number(&p)).is_some_and(|o| o != 0.0) {
        return Err("its offset from the mesh vertex is not decoded".into());
    }
    // Both coordinates are fractions of the width (sampled on a layer wider than tall).
    let rest = [vertex[0] * w, vertex[1] * w];
    if h > w {
        col.lose(
            LossCategory::Effects,
            format!("{here}: pin '{label}': its rest place on a layer taller than wide is scaled by the layer width (not sampled), it may sit elsewhere"),
        );
    }
    // After Effects stores a Scale and a Rotation on every Deform pin, a plain Position pin too
    // (the sample's five Position pins each carry Scale 100 and Rotation 0, keyed once). They are
    // read into an Advanced pin first; the pin falls back to a Position pin below when neither
    // ever leaves its neutral value.
    let (scale, rotation) = (prop(PIN_SCALE), prop(PIN_ROTATION));
    let kind = if scale.is_some() || rotation.is_some() { PinKind::Advanced } else { PinKind::Position };
    let mut g = pin_group(ids, label, kind, rest);
    for (node, id, k) in [(prop(PIN_POSITION), "position", Kind::EffectPoint(w, h)), (scale, "scale", Kind::Number), (rotation, "rotation", Kind::Number)] {
        let (Some(node), Some(target)) = (node, g.get_mut(id)) else { continue };
        if node.expression().is_some() {
            col.lose(LossCategory::Effects, format!("{here}: pin '{label}': the expression on {id} is not imported"));
        }
        if node.is_animated() {
            import_keys(&node, k, cx.layer, &format!("{label} {id}"), cx.tm, target, col);
        } else if let Some(v) = node.static_value().and_then(|v| k.value(&v)) {
            target.value = v;
        }
    }
    // A Scale that stays 100 % and a Rotation that stays 0 everywhere (static value, every key,
    // no expression) mean the pin is a plain Position pin: an Advanced pin would hold its
    // neighbourhood's rotation and bend the layer differently.
    let neutral = |id: &str, n: f64| g.get(id).is_none_or(|p| p.expr.is_none() && p.value.as_f64() == n && p.keys.iter().all(|k| k.value.as_f64() == n));
    if kind == PinKind::Advanced && neutral("scale", 100.0) && neutral("rotation", 0.0) {
        g.children.retain(|c| c.as_prop().is_none_or(|p| p.match_id != "scale" && p.match_id != "rotation"));
        if let Some(k) = g.get_mut("kind") {
            k.value = Value::Enum(PinKind::Position.index());
        }
    }
    Ok(g)
}

/// A one-component static value.
fn static_number(p: &AepProperty<'_, '_>) -> Option<f64> {
    p.static_value().and_then(|v| v.first().copied()).filter(|v| v.is_finite())
}

/// The vertices of a Puppet mesh's data (`fpms`), in fractions of the layer width. `None` unless
/// exactly one place in the data is a vertex count followed by that many vertices and a triangle
/// count whose index triples end the data (one pad byte may follow), every index naming one of the
/// vertices.
pub(crate) fn mesh_vertices(data: &[u8]) -> Option<Vec<[f64; 2]>> {
    let u32_at = |o: usize| data.get(o..o.checked_add(4)?).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
    let f64_at = |o: usize| data.get(o..o.checked_add(8)?).and_then(|b| b.try_into().ok()).map(f64::from_be_bytes);
    let mut found: Option<(usize, usize)> = None;
    for at in 0..data.len().saturating_sub(8) {
        let Some(v) = u32_at(at).map(|v| v as usize).filter(|v| (1..=MAX_VERTICES).contains(v)) else { continue };
        let Some(tri_at) = v.checked_mul(16).and_then(|b| b.checked_add(at)).and_then(|b| b.checked_add(4)) else { continue };
        let Some(t) = u32_at(tri_at).map(|t| t as usize).filter(|t| *t > 0) else { continue };
        let Some(end) = t.checked_mul(12).and_then(|b| b.checked_add(tri_at)).and_then(|b| b.checked_add(4)) else { continue };
        if end > data.len() || data.len() - end > 1 {
            continue;
        }
        let first = tri_at.saturating_add(4);
        if !(0..t.saturating_mul(3)).all(|i| u32_at(first.saturating_add(i.saturating_mul(4))).is_some_and(|x| (x as usize) < v)) {
            continue;
        }
        if found.is_some() {
            // Two readings: the data is not what this reader knows.
            return None;
        }
        found = Some((at, v));
    }
    let (at, v) = found?;
    (0..v)
        .map(|i| {
            let o = at.saturating_add(4).saturating_add(i.saturating_mul(16));
            let (x, y) = (f64_at(o)?, f64_at(o.saturating_add(8))?);
            (x.is_finite() && y.is_finite() && x.abs() <= 16.0 && y.abs() <= 16.0).then_some([x, y])
        })
        .collect()
}

/// `Mesh n (Triangles t, Expansion e): Deform 3 pins (1 keyed) [atom names], Overlap no pins, ...`
fn mesh_summary(mesh: &AepGroup<'_, '_>, n: usize, unknown: &mut Vec<String>) -> String {
    let name = mesh.user_name().unwrap_or_else(|| format!("Mesh {n}"));
    let mut settings: Vec<String> = Vec::new();
    let mut containers: Vec<String> = Vec::new();
    for e in mesh.entries() {
        let label = match e.match_name.as_str() {
            TRIANGLES => Some("Triangles"),
            EXPANSION => Some("Expansion"),
            _ => None,
        };
        match (label, e.node) {
            (Some(label), PropNode::Property(p)) => settings.push(format!("{label} {}", describe(&p))),
            // The triangulation After Effects keeps; EffectCraft builds its own.
            _ if e.match_name == MESH_DATA => {}
            _ => match (PIN_GROUPS.iter().find(|(m, _)| *m == e.match_name), e.node) {
                (Some((_, shown)), PropNode::Group(g)) => containers.push(format!("{shown} {}", pins_summary(&g, unknown))),
                _ => push_unique(unknown, e.match_name),
            },
        }
    }
    let settings = if settings.is_empty() { String::new() } else { format!(" ({})", settings.join(", ")) };
    let containers = if containers.is_empty() { "no pin groups".to_string() } else { containers.join(", ") };
    format!("{name}{settings}: {containers}")
}

/// `3 pins (1 keyed, 1 with an expression) [ADBE FreePin3 PosPin Atom]`, or `no pins`. An entry
/// of the container that is not a group (a pin is one) goes to `unknown`.
fn pins_summary(container: &AepGroup<'_, '_>, unknown: &mut Vec<String>) -> String {
    let mut count = 0usize;
    let mut keyed = 0usize;
    let mut expressions = 0usize;
    let mut atoms: Vec<String> = Vec::new();
    for e in container.entries() {
        let PropNode::Group(pin) = e.node else {
            push_unique(unknown, e.match_name);
            continue;
        };
        count = count.saturating_add(1);
        push_unique(&mut atoms, e.match_name);
        let props: Vec<AepProperty<'_, '_>> =
            pin.entries().into_iter().filter_map(|p| if let PropNode::Property(p) = p.node { Some(p) } else { None }).collect();
        if props.iter().any(AepProperty::is_animated) {
            keyed = keyed.saturating_add(1);
        }
        if props.iter().any(|p| p.expression().is_some()) {
            expressions = expressions.saturating_add(1);
        }
    }
    if count == 0 {
        return "no pins".to_string();
    }
    let mut what = vec![format!("{keyed} keyed")];
    if expressions > 0 {
        what.push(format!("{expressions} with an expression"));
    }
    format!("{count} pin{} ({}) [{}]", if count == 1 { "" } else { "s" }, what.join(", "), capped(&atoms, ", "))
}

/// A stored property's value for the report: `keyed`, the static value, or `unreadable`.
fn describe(p: &AepProperty<'_, '_>) -> String {
    if p.is_animated() {
        return "keyed".to_string();
    }
    match p.static_value() {
        Some(v) if v.len() == 1 => v.first().map(|x| format!("{x}")).unwrap_or_default(),
        Some(v) => format!("[{}]", v.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join(", ")),
        None => "unreadable".to_string(),
    }
}

/// Adds `name` to `names` unless it is there already.
fn push_unique(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

/// The first [`LIST_CAP`] of `items` joined by `sep`, and how many more there are.
fn capped(items: &[String], sep: &str) -> String {
    let shown: Vec<&str> = items.iter().take(LIST_CAP).map(String::as_str).collect();
    let mut out = shown.join(sep);
    let more = items.len().saturating_sub(LIST_CAP);
    if more > 0 {
        out.push_str(&format!("{sep}and {more} more"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::mesh_vertices;

    /// Mesh data with the outline block in front, `vertices` and `triangles`, and a pad byte.
    fn data(vertices: &[[f64; 2]], triangles: &[[u32; 3]]) -> Vec<u8> {
        let mut d = vec![2, 1, 0, 0, 0, 0, 0, 0, 0, 1];
        d.extend(0.5f32.to_be_bytes());
        d.extend(0.5f32.to_be_bytes());
        d.extend((vertices.len() as u32).to_be_bytes());
        for [x, y] in vertices {
            d.extend(x.to_be_bytes());
            d.extend(y.to_be_bytes());
        }
        d.extend((triangles.len() as u32).to_be_bytes());
        for t in triangles {
            for i in t {
                d.extend(i.to_be_bytes());
            }
        }
        d.push(0);
        d
    }

    #[test]
    fn reads_the_vertices_behind_the_outline() {
        let v = [[0.0, 0.0], [1.0, 0.0], [0.25, 0.5625]];
        assert_eq!(mesh_vertices(&data(&v, &[[0, 1, 2]])), Some(v.to_vec()));
        // Without the pad byte.
        let mut d = data(&v, &[[0, 1, 2]]);
        d.pop();
        assert_eq!(mesh_vertices(&d), Some(v.to_vec()));
    }

    #[test]
    fn refuses_data_that_does_not_add_up() {
        let v = [[0.0, 0.0], [1.0, 0.0], [0.25, 0.5625]];
        assert_eq!(mesh_vertices(&data(&v, &[[0, 1, 3]])), None, "an index past the vertices");
        assert_eq!(mesh_vertices(&data(&v, &[])), None, "no triangle");
        assert_eq!(mesh_vertices(&data(&[[f64::NAN, 0.0], [1.0, 0.0], [0.0, 1.0]], &[[0, 1, 2]])), None, "not a number");
        assert_eq!(mesh_vertices(&[]), None);
    }

    /// Truncated and bit-flipped mesh data returns vertices or `None`, never a panic.
    #[test]
    fn damaged_data_never_panics() {
        let d = data(&[[0.0, 0.0], [1.0, 0.0], [0.25, 0.5625], [0.5, 0.25]], &[[0, 1, 2], [0, 2, 3]]);
        for cut in 0..d.len() {
            let mut flipped = d.clone();
            if let Some(b) = flipped.get_mut(cut) {
                *b ^= 0xa5;
            }
            for bytes in [d.get(..cut).unwrap_or_default().to_vec(), flipped] {
                assert!(std::panic::catch_unwind(|| mesh_vertices(&bytes)).is_ok(), "panicked at byte {cut}");
            }
        }
        // A vertex count near u32::MAX does not allocate or overflow.
        let mut huge = data(&[[0.0, 0.0]], &[[0, 0, 0]]);
        if let Some(c) = huge.get_mut(18..22) {
            c.copy_from_slice(&u32::MAX.to_be_bytes());
        }
        assert!(std::panic::catch_unwind(|| mesh_vertices(&huge)).is_ok());
    }
}
