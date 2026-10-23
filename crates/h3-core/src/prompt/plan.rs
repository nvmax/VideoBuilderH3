//! Shot plans, cut styles and the instruction text built from them. Port of the first part of
//! `minimax/shot_prompt.py`. A cut plan is a loose JSON object (`Obj`), like the pack's dict, so the keys other code
//! adds to it survive.

use serde::Serialize;
use serde_json::{json, Value};

use crate::error::CoreError;
use crate::pyval::{float_or_zero, py_float_str, py_str, round_to, text_or_empty, truthy, Obj};

pub const HARD_LIMIT: usize = 7000;
pub const FALLBACK_SHOT: &str =
    "A steady cinematic shot continues the scene with natural camera movement and continuous physical action.";

pub const CONTINUOUS_CUTS_OPENING: &str = concat!(
    "All shots follow one continuous performance, preserving every person's exact position, body orientation, ",
    "posture, and ongoing movement across every camera cut. Each new shot continues directly from the final physical ",
    "moment of the previous shot, with only the camera angle and framing changing."
);

pub const CONTINUOUS_CUTS_RULES: &str = concat!(
    "CONTINUOUS MOMENT ACROSS CUTS — MANDATORY: All shots are one unbroken moment filmed from different camera angles. ",
    "Each later shot starts exactly where the previous shot ended: the same spot, the same position and body ",
    "orientation, the same wardrobe, props, light and weather, and the action already in progress. Only the camera ",
    "angle, distance and framing change at a cut. The action may move forward naturally (a step, a turn, a gesture ",
    "finishing), but never jumps in time, never resets a pose, and never moves anyone to a new place. Begin each later ",
    "description by naming the new framing and what it continues from, for example: 'the camera cuts to a low-angle ",
    "close-up of <Subject 1>, continuing her raised-hand gesture from the shot before'."
);

const CRAFT_ACTING: &str = concat!(
    "PERFORMANCE CRAFT — MANDATORY: Give every visible face its own concrete expression in every shot, ",
    "written as what the brows, eyes, smile, head and shoulders do, for example eyes narrowing, brows lifting, a slow smile spreading or the head turning away. ",
    "Take each emotion from this scene's lyric meaning, story beat and song energy rather than a stock mood, choose two specific emotions wherever an emotion header is written, ",
    "and let the expression change at least once in a longer shot. Scale every expression to the {style} video style and the scene's energy: ",
    "stylized, animated or high-energy scenes use big, exaggerated expressions; realistic or quiet scenes use small, precise ones. ",
    "Write each emotion as a visible action instead of a feeling word or a face colour: anger is a jabbed finger, gritted teeth or brows slammed down, never a red, flushed or pale face. ",
    "Faces stay dry, with no tears, crying, wet eyes, glistening skin or sweat."
);
const CRAFT_SILENT: &str = concat!(
    "SILENT PEOPLE: Name every visible person who is not singing or speaking at that moment and state that their lips stay pressed together, ",
    "with their own expression, for example '<Subject 2> watches with her lips pressed together and her brows lifted.'"
);
const CRAFT_FRAMING: &str = concat!(
    "FACES WHILE SINGING OR SPEAKING: Unless a per-shot framing above says otherwise, frame a person who is singing or speaking ",
    "from a medium shot to a close-up, with the camera near their eye level, so the face stays large in frame; keep wide and full-body framing for moments without vocals."
);
const CRAFT_CLEAR_FACE: &str = concat!(
    "Keep hands, drinks, food, hair and kisses away from a singing or speaking person's face while words are heard, ",
    "and finish any such gesture before the vocal starts."
);
const CRAFT_OPENING: &str =
    "OPENING AND ENDING: Open Shot 1 on the scene's main subject, and end the last shot on a clear held state such as a pose, a look or a finished gesture.";
const CRAFT_ACTION: &str = concat!(
    "ACTIONS AND PROPS: Give each shot one main action written as visible steps that reach a clear final state. ",
    "Once a prop is picked up, put down, handed over or put on, name it with where it is in every later shot of this scene."
);

/// One shot of a plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Shot {
    pub number: i64,
    pub time: f64,
    pub timecode: String,
}

/// `cut_plan.get("cut_times_seconds") or []` as numbers.
pub fn cut_times(plan: &Obj) -> Vec<f64> {
    plan.get("cut_times_seconds")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| float_or_zero(Some(v)).unwrap_or(0.0)).collect())
        .unwrap_or_default()
}

/// `float(plan.get("exact_duration_seconds") or 0)`.
pub fn exact_duration(plan: &Obj) -> f64 {
    float_or_zero(plan.get("exact_duration_seconds")).unwrap_or(0.0)
}

/// `MM:SS.mmm`. Seconds that round up to 60 print as `60.000`, like the pack.
pub fn timecode(seconds: Option<&Value>) -> Result<String, CoreError> {
    Ok(timecode_f(float_or_zero(seconds)?))
}

pub fn timecode_f(seconds: f64) -> String {
    let total = seconds.max(0.0);
    let minutes = (total / 60.0).floor();
    format!("{:02}:{:06.3}", minutes as i64, total - minutes * 60.0)
}

pub fn shot_plan(plan: &Obj) -> Vec<Shot> {
    let mut shots = vec![Shot { number: 1, time: 0.0, timecode: "00:00.000".to_string() }];
    for (i, t) in cut_times(plan).into_iter().enumerate() {
        shots.push(Shot { number: i as i64 + 2, time: t, timecode: timecode_f(t) });
    }
    shots
}

/// `continuous` or `free`.
pub fn cut_style_value(value: Option<&Value>) -> &'static str {
    if text_or_empty(value).trim().to_lowercase() == "continuous" { "continuous" } else { "free" }
}

/// A plan with cuts is marked `continuous` when the style asks for it.
pub fn apply_cut_style(plan: &Obj, cut_style: Option<&Value>) -> Obj {
    let mut out = plan.clone();
    let has_cuts = plan.get("cut_times_seconds").is_some_and(truthy);
    out.insert("continuous".into(), json!(has_cuts && cut_style_value(cut_style) == "continuous"));
    out
}

pub fn cut_plan_instruction(plan: &Obj) -> String {
    let text = frequency_cut_plan_instruction(plan);
    if plan.get("continuous").is_some_and(truthy) { format!("{text} {CONTINUOUS_CUTS_RULES}") } else { text }
}

fn frequency_cut_plan_instruction(plan: &Obj) -> String {
    let duration = py_float_str(round_to(exact_duration(plan), 3));
    let cuts = cut_times(plan);
    if cuts.is_empty() {
        return format!(
            concat!(
                "EDITING / CUT PLAN — MANDATORY: Use one smooth, continuous, uninterrupted shot for the full {}-second segment. ",
                "Output only [Shot 1]. Use no additional shot label, hard cut, angle reset, montage, dissolve, scene change, or transition. ",
                "Camera and character movement may develop inside the same continuous take."
            ),
            duration
        );
    }
    let times = cuts.iter().map(|t| timecode_f(*t)).collect::<Vec<_>>().join(", ");
    let plural = if cuts.len() == 1 { "" } else { "s" };
    format!(
        concat!(
            "EDITING / CUT PLAN — MANDATORY: Cut frequency {}/10 for this exact {}-second segment requires exactly ",
            "{} hard cut{} at {}, creating {} coherent shots. The builder will write [Shot 1] and every later ",
            "[Shot N] At MM:SS.mmm label. Return only the creative description for each shot. Each later description should stage a ",
            "continuity-preserving camera/shot cut. Do not omit, merge, add, or shift a scheduled cut. Do not add extra shots, montage beats, ",
            "dissolves, scene changes, or transitions outside this schedule.",
            "\n\n",
            "MULTI-SHOT CONTINUITY — MANDATORY: When a scene has multiple shots, write them as consecutive camera views of one uninterrupted physical performance in the same place and moment. Shot 1 establishes staging. In every later description, write a complete sentence explicitly continuing directly from the previous shot's final moment. At the instant of the cut, preserve the previous ending position, posture, body orientation, eyeline and prop contact, then carry the ongoing movement forward at its existing pace and direction. For a scene with active singing, for example: 'Continuing directly from the previous shot's final moment, <Subject 1> remains seated in the same position on the curb, her torso still angled toward the doorway, and continues the same uninterrupted singing performance in sync with the assigned audio.' Use the actual preceding physical state, not a generic continuity phrase. Do not restart an action, reset a pose, or replace continued performance with an unrelated beauty shot. An ongoing action may span scheduled cuts. Only camera angle, framing and camera movement change at a cut unless the scene explicitly requests a new action or transition. Show any requested physical change developing naturally from the inherited state. During every shot's active vocal interval, explicitly state that the assigned performer continues singing or speaking in sync with the assigned audio. Preserve cue timing and speaker assignments; instrumental, no-lip-sync and silent intervals remain non-vocal. Follow supplied lyric/dialogue allocations without duplicating an already quoted cue or restarting the scene's lyrics after a cut. Use complete grammatical sentences rather than fragments such as 'Still seated on the curb.' Self-contained shot prose and varied camera coverage must preserve the shared staging. Before returning, check every adjacent pair for a matching end-to-start physical state and continuous performance."
        ),
        py_str(plan.get("frequency")),
        duration,
        cuts.len(),
        plural,
        times,
        cuts.len() + 1
    )
}

/// Performance craft rules. A continued scene keeps its inherited framing and opening.
pub fn performance_craft_instruction(style: &str, has_cast: bool, has_vocals: bool, continuation: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    if has_cast {
        let named = if style.is_empty() { "chosen" } else { style };
        let named = if named.trim().is_empty() { "chosen" } else { named.trim() };
        lines.push(CRAFT_ACTING.replace("{style}", named));
        lines.push(CRAFT_SILENT.to_string());
        if has_vocals && !continuation {
            lines.push(CRAFT_FRAMING.to_string());
        }
        if has_vocals {
            lines.push(CRAFT_CLEAR_FACE.to_string());
        }
    }
    if !continuation {
        lines.push(CRAFT_OPENING.to_string());
    }
    lines.push(CRAFT_ACTION.to_string());
    lines.join("\n")
}
