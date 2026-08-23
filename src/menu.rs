//! Interactive component-selection wizard. Port of `interactiveMenu`.

use crate::app;
use crate::components::build_components;
use crate::console;
use crate::util::{atoi_prefix, to_lower, trim};

/// Port of `interactiveMenu`. Returns when the user starts ('y') or quits
/// ('q'/EOF); quit state is stored in the global user-quit flag.
pub fn interactive_menu() {
    let comps = build_components();
    console::out("\nNVIDIA post-install debloater\n");
    console::out("Logs will be written next to this executable.\n");
    console::out("Dry-run is recommended before destructive execution.\n\n");

    loop {
        let opts = app::opts(|o| o.clone());
        console::out(&format!(
            "Mode: {}\n",
            if opts.execute { "EXECUTE" } else { "DRY RUN" }
        ));
        console::out(&format!(
            "Preserve NVIDIA Container processes: {}\n",
            if opts.preserve_nv_containers {
                "yes"
            } else {
                "no"
            }
        ));
        console::out(&format!(
            "Kill lockers: {}\n",
            if opts.kill_lockers { "yes" } else { "no" }
        ));
        console::out(&format!(
            "Disable services: {}\n",
            if opts.disable_services { "yes" } else { "no" }
        ));
        console::out(&format!(
            "Disable scheduled tasks: {}\n",
            if opts.disable_scheduled_tasks {
                "yes"
            } else {
                "no"
            }
        ));
        console::out(&format!(
            "Delete scheduled tasks: {}\n",
            if opts.delete_scheduled_tasks {
                "yes"
            } else {
                "no"
            }
        ));
        console::out(&format!(
            "Schedule locked files for reboot deletion: {}\n\n",
            if opts.schedule_locked_for_reboot {
                "yes"
            } else {
                "no"
            }
        ));

        for (i, c) in comps.iter().enumerate() {
            let on = app::enabled(|m| m.get(&c.key).copied().unwrap_or(false));
            console::out(&format!(
                "{:>2}) [{}] {} - {}{}\n",
                i + 1,
                if on { "x" } else { " " },
                c.key,
                c.display_name,
                if c.optional { " (optional)" } else { "" }
            ));
        }
        console::out(
            "\nCommands:\n\
             \x20 number = toggle component\n\
             \x20 e = toggle execute/dry-run\n\
             \x20 k = toggle kill-lockers\n\
             \x20 s = toggle disable-services\n\
             \x20 d = toggle disable-scheduled-tasks\n\
             \x20 t = toggle delete-scheduled-tasks\n\
             \x20 r = toggle reboot-delete for locked files\n\
             \x20 c = toggle preserve NVIDIA containers\n\
             \x20 y = start\n\
             \x20 q = quit\n> ",
        );

        let Some(input_raw) = console::read_line() else {
            // EOF / closed stdin (e.g. non-interactive use): avoid a busy loop.
            console::out("\nInput closed; exiting.\n");
            app::set_user_quit_requested(true);
            return;
        };
        let input = trim(&input_raw);
        if input.is_empty() {
            continue;
        }
        let low = to_lower(&input);
        match low.as_str() {
            "q" => {
                app::set_user_quit_requested(true);
                return;
            }
            "y" => {
                if opts.execute {
                    console::out(
                        "EXECUTE mode will really delete/disable items. Type EXECUTE to confirm: ",
                    );
                    console::flush();
                    let confirmed = console::read_line().is_some_and(|l| trim(&l) == "EXECUTE");
                    if !confirmed {
                        console::out("Confirmation failed; staying in menu.\n\n");
                        continue;
                    }
                }
                break;
            }
            "e" => app::opts_mut(|o| o.execute = !o.execute),
            "k" => app::opts_mut(|o| o.kill_lockers = !o.kill_lockers),
            "s" => app::opts_mut(|o| o.disable_services = !o.disable_services),
            "d" => app::opts_mut(|o| o.disable_scheduled_tasks = !o.disable_scheduled_tasks),
            "t" => app::opts_mut(|o| o.delete_scheduled_tasks = !o.delete_scheduled_tasks),
            "r" => app::opts_mut(|o| o.schedule_locked_for_reboot = !o.schedule_locked_for_reboot),
            "c" => app::opts_mut(|o| o.preserve_nv_containers = !o.preserve_nv_containers),
            _ => {
                let n = atoi_prefix(&input);
                if n >= 1 && n as usize <= comps.len() {
                    let key = &comps[(n - 1) as usize].key;
                    app::enabled_mut(|m| {
                        if let Some(v) = m.get_mut(key.as_str()) {
                            *v = !*v;
                        }
                    });
                }
            }
        }
        console::out("\n");
    }
}
