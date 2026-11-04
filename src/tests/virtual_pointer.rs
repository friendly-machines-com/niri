use niri_config::Config;
use wayland_client::protocol::wl_pointer;

use crate::layout::HitType;

use super::*;

#[test]
fn axis_discrete_overflow() {
    let mut f = Fixture::new();
    let id = f.add_client();

    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let pointer = manager.create_virtual_pointer(None, &client.qh, ());
    pointer.axis_discrete(0, wl_pointer::Axis::VerticalScroll, 0., i32::MAX);
    f.roundtrip(id);
}

#[test]
fn tab_right_click_action() {
    // An explicit mouse binding wins over the tab action. Without either setting,
    // right-click still activates the tab without requesting a close.
    for (action, binds, closes, activates) in [
        ("", "", false, true),
        ("right-click-action { close-window; }", "", true, true),
        (
            "right-click-action { close-window; }",
            "binds { MouseRight { noop; }; }",
            false,
            false,
        ),
    ] {
        let config = Config::parse_mem(&format!(
            "layout {{ tab-indicator {{ position \"top\"; width 32; place-within-column; {action}; }}; }}\n{binds}"
        ))
        .unwrap();
        let mut f = Fixture::with_config(config);
        f.add_output(1, (1280, 720));
        let id = f.add_client();
        let mut surfaces = Vec::new();
        let mut windows = Vec::new();
        for _ in 0..2 {
            let window = f.client(id).create_window();
            let surface = window.surface.clone();
            window.commit();
            f.roundtrip(id);
            let window = f.client(id).window(&surface);
            window.attach_new_buffer();
            window.set_size(400, 400);
            window.ack_last_and_commit();
            f.double_roundtrip(id);
            windows.push(f.niri().layout.focus().unwrap().window.clone());
            surfaces.push(surface);
        }

        f.niri().layout.activate_window(&windows[0]);
        f.niri().layout.consume_into_column();
        f.niri().layout.toggle_column_tabbed_display();
        f.niri().layout.activate_window(&windows[1]);
        f.niri_complete_animations();

        // Find the inactive tab through production hit-testing instead of relying on
        // particular gap, border, or column-width defaults.
        let mut tab = None;
        'search: for y in (0..720).step_by(4) {
            for x in (0..1280).step_by(4) {
                let contents = f.niri().contents_under((f64::from(x), f64::from(y)).into());
                if let Some((
                    window,
                    HitType::Activate {
                        is_tab_indicator: true,
                    },
                )) = contents.window
                {
                    if window == windows[0] {
                        tab = Some((x, y));
                        break 'search;
                    }
                }
            }
        }
        let (x, y) = tab.expect("inactive tab must be clickable");
        let client = f.client(id);
        let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
        let pointer = manager.create_virtual_pointer(None, &client.qh, ());
        pointer.motion_absolute(0, x as u32, y as u32, 1280, 720);
        pointer.frame();
        f.roundtrip(id);
        pointer.button(1, 0x111, wl_pointer::ButtonState::Pressed);
        pointer.frame();
        f.double_roundtrip(id);
        assert_eq!(f.client(id).window(&surfaces[0]).close_requested, closes);
        assert!(!f.client(id).window(&surfaces[1]).close_requested);
        if !action.is_empty() {
            assert!(f.niri().suppressed_buttons.contains(&0x111));
        }
        let expected = if activates { &windows[0] } else { &windows[1] };
        assert_eq!(&f.niri().layout.focus().unwrap().window, expected);
        pointer.button(2, 0x111, wl_pointer::ButtonState::Released);
        pointer.frame();
        f.double_roundtrip(id);
        assert!(!f.niri().suppressed_buttons.contains(&0x111));
    }
}
