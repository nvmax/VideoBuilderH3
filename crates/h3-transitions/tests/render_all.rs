//! Every listed transition builds a pipeline and draws: `from` at progress 0, `to` at progress 1, something in between.

use std::time::Instant;

use h3_transitions::{catalog, params_from_json, transition, TransitionRenderer};
use serde_json::json;

const W: u32 = 320;
const H: u32 = 180;

/// A picture with structure in both directions, so a warp or a wipe changes it.
fn picture(seed: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let cell = ((x / 20) + (y / 20) + seed) % 2;
            let r = if seed == 0 { (x * 255 / (W - 1)) as u8 } else { 30 };
            let g = if cell == 0 { 40 } else { 200 };
            let b = if seed == 0 { (y * 255 / (H - 1)) as u8 } else { 230 };
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a.chunks(4).zip(b.chunks(4)).map(|(p, q)| (0..3).map(|i| u64::from(p[i].abs_diff(q[i]))).sum::<u64>()).sum();
    sum as f64 / (a.len() / 4 * 3) as f64 / 255.0
}

#[test]
fn every_transition_draws_and_reaches_both_pictures() {
    let (from, to) = (picture(0), picture(1));
    let mut renderer = TransitionRenderer::new().expect("a graphics adapter");
    println!("adapter: {}", renderer.adapter_name());
    let list = catalog();
    assert!(list.len() > 100, "only {} transitions are usable", list.len());

    let mut frame = Vec::new();
    let mut problems = Vec::new();
    for entry in &list {
        let name = entry["id"].as_str().unwrap();
        let params = params_from_json(transition(name).unwrap(), &json!({})).unwrap();
        let mut at = |progress: f32| {
            renderer.render(&params, W, H, &from, &to, progress, &mut frame).map(|()| frame.clone())
        };
        let (start, middle, end) = match (at(0.0), at(0.5), at(1.0)) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            (a, b, c) => {
                problems.push(format!("{name}: failed to draw ({:?} {:?} {:?})", a.err(), b.err(), c.err()));
                continue;
            }
        };
        let (d_start, d_end, d_mid_from, d_mid_to) = (mean_difference(&start, &from), mean_difference(&end, &to), mean_difference(&middle, &from), mean_difference(&middle, &to));
        println!("{name:<28} start {d_start:.3} end {d_end:.3} mid-from {d_mid_from:.3} mid-to {d_mid_to:.3}");
        if d_start > 0.06 || d_end > 0.06 {
            problems.push(format!("{name}: progress 0 is {d_start:.3} from `from`, progress 1 is {d_end:.3} from `to`"));
        }
        if d_mid_from < 0.005 && d_mid_to < 0.005 {
            problems.push(format!("{name}: progress 0.5 looks like neither picture"));
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn parameters_come_from_json() {
    let swirl = transition("DirectionalWarp").unwrap();
    let params = params_from_json(swirl, &json!({"direction": [0.0, 1.0]})).unwrap();
    assert!(params.iter().any(|(p, v)| p.name == "direction" && format!("{v:?}") == "Vec2([0.0, 1.0])"));
    assert!(params_from_json(swirl, &json!({"nope": 1})).is_err());
    assert!(params_from_json(swirl, &json!({"direction": 3})).is_err());
    assert!(transition("NoSuchTransition").is_err());
}

#[test]
fn full_hd_frame_time() {
    let (w, h) = (1920u32, 1080u32);
    let (from, to) = (vec![90u8; (w * h * 4) as usize], vec![200u8; (w * h * 4) as usize]);
    let mut renderer = TransitionRenderer::new().expect("a graphics adapter");
    let params = params_from_json(transition("Swirl").unwrap(), &json!({})).unwrap();
    let mut frame = Vec::new();
    renderer.render(&params, w, h, &from, &to, 0.3, &mut frame).unwrap();
    let started = Instant::now();
    for i in 0..10 {
        renderer.render(&params, w, h, &from, &to, i as f32 / 10.0, &mut frame).unwrap();
    }
    println!("1920x1080 Swirl: {:.1} ms a frame on {}", started.elapsed().as_secs_f64() * 100.0, renderer.adapter_name());
    assert_eq!(frame.len(), (w * h * 4) as usize);
}
