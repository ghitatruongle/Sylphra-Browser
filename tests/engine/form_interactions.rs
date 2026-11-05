use sylphra::css_parser::parse_css;
use sylphra::web_runtime::PageRuntime;

const PAGE_CSS: &str =
    "input{width:120px;height:24px}button{width:60px;height:24px}select{width:90px}";

fn runtime_with_form() -> PageRuntime {
    let dom = sylphra::parser::parse_html(
        "<form action='/search' method='get'>\
         <input name='q'>\
         <select name='lang'><option value='vi' selected>Vi</option><option value='en'>En</option></select>\
         <button type='submit'>Go</button>\
         </form>",
    );
    PageRuntime::from_element(&dom, parse_css(PAGE_CSS), 800, "https://a.com/page").unwrap()
}

#[test]
fn clicking_input_focuses_it_and_typing_updates_value() {
    let mut runtime = runtime_with_form();
    let input = runtime.query_node("input").unwrap();
    runtime.click(input).unwrap();
    assert_eq!(runtime.focused_node(), Some(input));

    runtime.dispatch_keyboard("keydown", "a").unwrap();
    let submission = {
        let button = runtime.query_node("button").unwrap();
        runtime.click(button).unwrap();
        runtime.take_pending_submission()
    };
    let submission = submission.expect("submitting the form produces a GET url");
    assert!(
        submission.contains("q=a"),
        "typed value must be submitted, got {}",
        submission
    );
}

#[test]
fn select_click_advances_selected_option() {
    let mut runtime = runtime_with_form();
    let select = runtime.query_node("select").unwrap();
    runtime.click(select).unwrap();
    let submission = {
        let button = runtime.query_node("button").unwrap();
        runtime.click(button).unwrap();
        runtime.take_pending_submission()
    };
    let submission = submission.expect("submit after select interaction");
    assert!(
        submission.contains("lang=en"),
        "select cycling should submit the next option, got {}",
        submission
    );
}

#[test]
fn backspace_removes_last_character_before_submit() {
    let mut runtime = runtime_with_form();
    let input = runtime.query_node("input").unwrap();
    runtime.click(input).unwrap();
    runtime.dispatch_keyboard("keydown", "a").unwrap();
    runtime.dispatch_keyboard("keydown", "b").unwrap();
    runtime.dispatch_keyboard("keydown", "Backspace").unwrap();
    let button = runtime.query_node("button").unwrap();
    runtime.click(button).unwrap();
    let submission = runtime.take_pending_submission().unwrap();
    assert!(
        submission.contains("q=a"),
        "backspace should trim to 'a', got {}",
        submission
    );
}

#[test]
fn required_empty_field_blocks_submission() {
    let dom = sylphra::parser::parse_html(
        "<form action='/go' method='get'><input name='q' required><button type='submit'>Go</button></form>",
    );
    let mut runtime =
        PageRuntime::from_element(&dom, parse_css(PAGE_CSS), 800, "https://a.com/page").unwrap();
    let button = runtime.query_node("button").unwrap();
    runtime.click(button).unwrap();
    assert!(
        runtime.take_pending_submission().is_none(),
        "invalid required field must block the submission"
    );
}

#[test]
fn display_list_exposes_form_control_regions() {
    let dom = sylphra::parser::parse_html(
        "<form><input name='q' value='cats'><button type='submit'>Go</button></form>",
    );
    let rules = parse_css(PAGE_CSS);
    let mut live = sylphra::LiveDocument::from_element(&dom, rules.clone(), 800);
    let render = live.refresh().clone();
    let layout = sylphra::layout::create_layout_tree(&render.dom, &rules, 800).unwrap();
    let list = sylphra::paint::build_display_list(&layout);
    assert!(
        list.form_controls.iter().any(|region| region.kind
            == sylphra::paint::FormControlKind::TextInput
            && region.value == "cats"),
        "input region carries its value"
    );
    assert!(
        list.form_controls
            .iter()
            .any(|region| region.kind == sylphra::paint::FormControlKind::Button),
        "button region is emitted"
    );
}

#[test]
fn enter_in_text_input_submits_the_form() {
    let mut runtime = runtime_with_form();
    let input = runtime.query_node("input").unwrap();
    runtime.click(input).unwrap();
    runtime.dispatch_keyboard("keydown", "a").unwrap();
    runtime.dispatch_keyboard("keydown", "Enter").unwrap();
    let submission = runtime
        .take_pending_submission()
        .expect("enter submits the form");
    assert!(
        submission.contains("q=a"),
        "enter submission carries the value, got {}",
        submission
    );
}
