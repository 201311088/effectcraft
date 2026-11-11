//! After Effects' Puppet (`ADBE FreePin3`, #3093) arrives as EffectCraft's Puppet
//! (`ec.distort.puppet`) in its place in the stack, and what its meshes and pins hold is reported
//! with its meshes and Deform pins; what it cannot take is reported. The effect is built into the
//! committed `synthetic.aep` in place of its Glow (`fx_inject`, in memory; the layer is 8x8), in
//! the tree docs/aep-format.md 3.7 gives for Puppet, with mesh data (`fpms`) laid out as After
//! Effects 24.6 writes it (see the `import::puppet` module docs; a real project's pins were checked
//! against it to the pixel, but its file names a person's media, so it is not committed).

use effectcraft_aep::import::{ImportOptions, LossCategory, import};
use effectcraft_aep::{Body, Chunk};
use effectcraft_keyframe::Value;
use effectcraft_project::{Project, PropGroup};

use crate::import::fx_inject::{bytes_with_effect, import_bytes, losses, nested, record, tdmn, utf8};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");
const PUPPET: &str = "ADBE FreePin3";
const POS_ATOM: &str = "ADBE FreePin3 PosPin Atom";

fn tdsb(v: u32) -> Chunk<'static> {
    Chunk::data(*b"tdsb", v.to_be_bytes().to_vec())
}

/// A property group of `entries` (match name, node).
fn group(entries: Vec<(&str, Chunk<'static>)>) -> Chunk<'static> {
    let mut kids = vec![tdsb(1)];
    for (name, node) in entries {
        kids.push(tdmn(name));
        kids.push(node);
    }
    Chunk::list(*b"tdgp", kids)
}

/// A static two-component value record (`record`'s header with two components).
fn record2(t: &Chunk<'_>, x: f64, y: f64) -> Chunk<'static> {
    with_components(record(t, x), 2, false, |cdat| cdat[8..16].copy_from_slice(&y.to_be_bytes()))
}

/// A keyed two-component spatial record, linear keys `(time units, [x, y])`: 104-byte records with
/// the value from byte 56, as After Effects writes a pin's Position.
fn keyed2(t: &Chunk<'_>, keys: &[(i32, [f64; 2])]) -> Chunk<'static> {
    let first = keys.first().map_or([0.0; 2], |k| k.1);
    let mut lhd3 = vec![0u8; 52];
    lhd3[0..4].copy_from_slice(&0x00d0_0beeu32.to_be_bytes());
    lhd3[8..12].copy_from_slice(&(keys.len() as u32).to_be_bytes());
    lhd3[16..20].copy_from_slice(&104u32.to_be_bytes());
    let mut ldat = Vec::new();
    for (time, [x, y]) in keys {
        let mut rec = vec![0u8; 104];
        rec[0..4].copy_from_slice(&time.to_be_bytes());
        rec[4] = 1;
        rec[5] = 1;
        rec[56..64].copy_from_slice(&x.to_be_bytes());
        rec[64..72].copy_from_slice(&y.to_be_bytes());
        ldat.extend(rec);
    }
    let rec = with_components(record2(t, first[0], first[1]), 2, true, |_| {});
    let Body::List { mut children, .. } = rec.body else { return rec };
    children.push(Chunk::list(*b"list", vec![Chunk::data(*b"lhd3", lhd3), Chunk::data(*b"ldat", ldat)]));
    Chunk::list(*b"tdbs", children)
}

/// `rec` with `tdb4` saying `components` (byte 3) and spatial (byte 7), and its `cdat` edited.
fn with_components(rec: Chunk<'static>, components: u8, spatial: bool, edit: impl Fn(&mut Vec<u8>)) -> Chunk<'static> {
    let Body::List { children, .. } = rec.body else { return rec };
    let kids = children
        .into_iter()
        .map(|c| match (c.id, c.body) {
            (id, Body::Data(d)) if id == *b"tdb4" => {
                let mut d = d.to_vec();
                d[3] = components;
                d[7] = u8::from(spatial);
                Chunk::data(id, d)
            }
            (id, Body::Data(d)) if id == *b"cdat" => {
                let mut d = d.to_vec();
                edit(&mut d);
                Chunk::data(id, d)
            }
            (id, body) => Chunk { id, body, pad: None },
        })
        .collect();
    Chunk::list(*b"tdbs", kids)
}

/// Mesh data (`fpms`) as After Effects writes it: an outline block this reader skips, the vertex
/// count and vertices (fractions of the layer width), the triangle count and index triples, a pad
/// byte.
fn fpms(vertices: &[[f64; 2]], triangles: &[[u32; 3]]) -> Vec<u8> {
    let mut d = vec![2, 1, 0, 0, 0, 0];
    // Outline: three float pairs after their count.
    d.extend(3u32.to_be_bytes());
    for v in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
        d.extend(v.to_be_bytes());
    }
    d.extend([0xad, 0x0b, 0xeb, 0x0a, 0x00]);
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

/// The vertices of the test mesh, in fractions of the (8 px) layer width.
const VERTICES: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.5, 0.25], [0.25, 0.75]];

/// A Deform pin naming mesh vertex `index`, with `position` and, when `advanced`, Scale and Rotation.
fn pin(t: &Chunk<'_>, index: f64, position: Chunk<'static>, advanced: bool) -> (&'static str, Chunk<'static>) {
    let mut entries = vec![("ADBE FreePin3 PosPin Vtx Index", record(t, index)), ("ADBE FreePin3 PosPin Position", position)];
    if advanced {
        entries.push(("ADBE FreePin3 PosPin Scale", record(t, 150.0)));
        entries.push(("ADBE FreePin3 PosPin Rotation", record(t, 30.0)));
    }
    (POS_ATOM, group(entries))
}

/// `Mesh 1` with mesh data `data`: Triangles 500, Expansion 5, Deform pins `pins` plus an atom
/// name nobody knows, one Stiffness pin, and a group nobody knows.
fn mesh(t: &Chunk<'_>, data: Option<Vec<u8>>, pins: Vec<(&'static str, Chunk<'static>)>) -> Chunk<'static> {
    let mut deform = pins;
    deform.push(("ADBE FreePin3 Test Atom", group(vec![])));
    let starch = group(vec![("ADBE FreePin3 StarchPin Atom", group(vec![("ADBE FreePin3 StarchPin Amount", record(t, 0.5))]))]);
    let mesh_record = Chunk::list(*b"tdbs", vec![tdsb(3)]);
    let mut kids = vec![tdsb(1), nested(*b"tdsn", vec![utf8("Mesh 1")]), tdmn("ADBE FreePin3 Mesh")];
    if let Some(d) = data {
        kids.push(Chunk::data(*b"fpmv", 6u32.to_be_bytes().to_vec()));
        kids.push(Chunk::data(*b"fpsz", (d.len() as u32).to_be_bytes().to_vec()));
        kids.push(Chunk::data(*b"fpms", d));
    }
    kids.push(mesh_record);
    for (name, node) in [
        ("ADBE FreePin3 Mesh Tri Count", record(t, 500.0)),
        ("ADBE FreePin3 Mesh Expansion", record(t, 5.0)),
        ("ADBE FreePin3 PosPins", group(deform)),
        ("ADBE FreePin3 HghtPins", group(vec![])),
        ("ADBE FreePin3 StarchPins", starch),
        ("ADBE FreePin3 Test Group", group(vec![])),
    ] {
        kids.push(tdmn(name));
        kids.push(node);
    }
    Chunk::list(*b"tdgp", kids)
}

/// The mesh tree (`arap`) holding `meshes`.
fn arap(meshes: Vec<Chunk<'static>>) -> Chunk<'static> {
    let meshes = meshes.into_iter().map(|m| ("ADBE FreePin3 Mesh Atom", m)).collect();
    group(vec![("ADBE FreePin3 Outlines", group(vec![])), ("ADBE FreePin3 Mesh Group", group(meshes))])
}

/// The pins of the first mesh of an EffectCraft Puppet.
fn ec_pins(g: &PropGroup) -> Vec<PropGroup> {
    effectcraft_effects::puppet::meshes(g).flat_map(effectcraft_effects::puppet::pins).cloned().collect()
}

fn vec2(g: &PropGroup, id: &str) -> [f64; 2] {
    match g.get(id).map(|p| &p.value) {
        Some(Value::Vec2(v)) => *v,
        other => panic!("{id}: {other:?}"),
    }
}
/// The Puppet instance: `fx_switch` is the value group's `tdsb`, `arap` its mesh tree (`None`:
/// none), `extra` more entries of its value group.
fn sspc(fx_switch: u32, arap: Option<Chunk<'static>>, extra: Vec<(&str, Chunk<'static>)>) -> Chunk<'static> {
    let pard = |kind: u8| {
        let mut b = vec![0u8; 148];
        b[15] = kind;
        Chunk::data(*b"pard", b)
    };
    let part = vec![Chunk::data(*b"parn", 2u32.to_be_bytes().to_vec()), tdmn("ADBE FreePin3-0000"), pard(0), tdmn("ADBE Effect Built In Params"), pard(9)];
    let mut values = vec![tdsb(fx_switch)];
    if let Some(arap) = arap {
        values.push(tdmn("ADBE FreePin3 ARAP Group"));
        values.push(arap);
    }
    for (name, node) in extra {
        values.push(tdmn(name));
        values.push(node);
    }
    Chunk::list(*b"sspc", vec![nested(*b"fnam", vec![utf8("Puppet")]), Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values)])
}

/// The layer that holds `effect`, the effect's place in that layer's stack and the stack's size.
fn stack_place(bytes: &[u8], effect: &str) -> Option<(String, usize, usize)> {
    let mut project = Project::default();
    let none = |_: &str| false;
    import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    project.comps().flat_map(|(_, c)| c.layers.iter()).find_map(|l| {
        let fx: Vec<_> = l.props.sub("effects")?.children.iter().filter_map(|n| n.as_group()).collect();
        let at = fx.iter().position(|g| g.match_id == effect)?;
        Some((l.name.clone(), at, fx.len()))
    })
}

#[test]
fn deform_pins_arrive_at_their_mesh_vertices() {
    let bytes = bytes_with_effect(PUPPET, |t| {
        let pins = vec![
            // Vertex 2 (0.5, 0.25 of the width): a static Position there.
            pin(t, 2.0, record2(t, 0.5, 0.25), false),
            // Vertex 3 (0.25, 0.75): keyed, Advanced.
            pin(t, 3.0, keyed2(t, &[(0, [0.25, 0.75]), (25_600, [0.5, 0.5])]), true),
            // No vertex 9 in a four-vertex mesh.
            pin(t, 9.0, record2(t, 0.5, 0.5), false),
        ];
        let data = fpms(&VERTICES, &[[0, 1, 2], [0, 2, 3]]);
        sspc(1, Some(arap(vec![mesh(t, Some(data), pins)])), vec![("ADBE FreePin3 On Transparent", record(t, 1.0))])
    });
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    let g = g.expect("an EffectCraft Puppet in the Glow's place");
    assert!(g.enabled);
    assert_eq!(g.name, "Puppet");
    assert_eq!(stack_place(&bytes, effectcraft_effects::puppet::ID), stack_place(SYNTHETIC, "ec.stylize.glow"));

    let meshes: Vec<_> = effectcraft_effects::puppet::meshes(&g).collect();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].name, "Mesh 1");
    assert_eq!(meshes[0].get("triangles").map(|p| p.value.clone()), Some(Value::Scalar(500.0)));
    assert_eq!(meshes[0].get("expansion").map(|p| p.value.clone()), Some(Value::Scalar(5.0)));

    let pins = ec_pins(&g);
    assert_eq!(pins.len(), 2, "the pin of a vertex outside the mesh is not made up");
    let (a, b) = (&pins[0], &pins[1]);
    // Rest places: the vertex times the layer width (8 px).
    assert_eq!(vec2(a, "rest"), [4.0, 2.0]);
    assert_eq!(vec2(a, "position"), [4.0, 2.0]);
    assert_eq!(a.get("kind").map(|k| k.value.clone()), Some(Value::Enum(effectcraft_effects::puppet::PinKind::Position.index())));
    assert_eq!(vec2(b, "rest"), [2.0, 6.0]);
    assert_eq!(b.get("kind").map(|k| k.value.clone()), Some(Value::Enum(effectcraft_effects::puppet::PinKind::Advanced.index())));
    let keys: Vec<Value> = b.get("position").map(|p| p.keys.iter().map(|k| k.value.clone()).collect()).unwrap_or_default();
    assert_eq!(keys, vec![Value::Vec2([2.0, 6.0]), Value::Vec2([4.0, 4.0])]);
    assert_eq!(b.get("scale").map(|p| p.value.clone()), Some(Value::Scalar(150.0)));
    assert_eq!(b.get("rotation").map(|p| p.value.clone()), Some(Value::Scalar(30.0)));

    let l = losses(&r, LossCategory::Effects, PUPPET);
    assert!(l.iter().any(|d| d.ends_with("property ADBE FreePin3 On Transparent (1) not imported")), "{l:#?}");
    assert!(l.iter().any(|d| d.contains("pin 'Puppet Pin 3' not imported (mesh vertex 9 is not in the mesh (4 vertices))")), "{l:#?}");
    assert!(l.iter().any(|d| d.ends_with("Deform entry ADBE FreePin3 Test Atom not imported")), "{l:#?}");
    assert!(l.iter().any(|d| d.ends_with("Stiffness 1 pin (0 keyed) [ADBE FreePin3 StarchPin Atom] not imported")), "{l:#?}");
    assert!(l.iter().any(|d| d.ends_with("entry ADBE FreePin3 Test Group not imported")), "{l:#?}");
    assert!(!l.iter().any(|d| d.contains("imported without")), "the mesh arrived: {l:#?}");
    assert_eq!(losses(&r, LossCategory::DamagedData, PUPPET), Vec::<String>::new());
}

#[test]
fn a_mesh_whose_data_is_missing_or_does_not_add_up_is_reported() {
    let bytes = bytes_with_effect(PUPPET, |t| {
        let p = || vec![pin(t, 0.0, record2(t, 0.0, 0.0), false)];
        let mut short = fpms(&VERTICES, &[[0, 1, 2]]);
        short.truncate(short.len() - 6);
        let bad_index = fpms(&VERTICES, &[[0, 1, 7]]);
        sspc(1, Some(arap(vec![mesh(t, None, p()), mesh(t, Some(short), p()), mesh(t, Some(bad_index), p())])), vec![])
    });
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    assert_eq!(ec_pins(&g.expect("the effect still arrives")).len(), 0);
    let l = losses(&r, LossCategory::Effects, PUPPET);
    let without: Vec<_> = l.iter().filter(|d| d.contains("imported without Mesh 1")).collect();
    assert_eq!(without.len(), 3, "{l:#?}");
    assert!(without[0].ends_with("(its mesh data is missing)"), "{}", without[0]);
    assert!(without[1].ends_with("(its mesh data could not be read)"), "{}", without[1]);
    assert!(without[2].ends_with("(its mesh data could not be read)"), "{}", without[2]);
}

/// After Effects keeps the auto-traced outline of a Puppet nobody pinned, and no mesh group: the
/// import used to report it as "imported without meshes or pins" although it held none.
#[test]
fn a_puppet_with_an_outline_and_no_mesh_group_loses_nothing() {
    let bytes = bytes_with_effect(PUPPET, |_| sspc(1, Some(group(vec![("ADBE FreePin3 Outlines", group(vec![]))])), vec![]));
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    assert!(g.is_some());
    assert_eq!(losses(&r, LossCategory::Effects, PUPPET), Vec::<String>::new());
}
#[test]
fn a_puppet_without_a_mesh_loses_nothing_and_keeps_its_fx_switch() {
    let bytes = bytes_with_effect(PUPPET, |_| sspc(0, Some(group(vec![("ADBE FreePin3 Mesh Group", group(vec![]))])), vec![]));
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    let g = g.expect("an EffectCraft Puppet");
    assert!(!g.enabled, "switched off in the Effect Controls");
    assert_eq!(losses(&r, LossCategory::Effects, PUPPET), Vec::<String>::new());
}

#[test]
fn a_puppet_whose_mesh_tree_is_not_found_is_reported() {
    let bytes = bytes_with_effect(PUPPET, |t| sspc(1, None, vec![("ADBE FreePin3 Other Group", group(vec![("x", record(t, 1.0))]))]));
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    assert!(g.is_some(), "the effect still arrives");
    let l = losses(&r, LossCategory::Effects, PUPPET);
    assert_eq!(l.len(), 1, "{l:#?}");
    assert!(
        l[0].ends_with(
            "imported without meshes or pins: its mesh group (ADBE FreePin3 ARAP Group / ADBE FreePin3 Mesh Group) was not found or is not a group; \
             not recognised: ADBE FreePin3 Other Group"
        ),
        "{}",
        l[0]
    );
}
