# Task progress UI coverage

Run `cargo test --test gjs` and `cargo test`, then
`ISC_FUNCTIONS_DIR=/home/mk/projects/linux_installation_scripts_functions bash tests/test_install.sh`.
The gjs probes use mock actors, so no windows or system actions are launched.

| Input / contract | Saved assertion |
| --- | --- |
| Separate 25% and 75% task fractions | model_status_task_progress_layer_updates_in_place: each fill width follows its own 400px track. |
| Progress behind text | Same case: stack children are background first and text second. |
| Track resize | Same case: allocation changes to 200px updates a 25% fill to 50px. |
| Confirmed completion and unread glow | Same case: track invisible as ams-agent-unread appears; read DONE also stays hidden. |
| Follow-up running turn | Same case: new fraction reappears and completion class is removed. |
| Missing/bad progress and boundaries | model_status_task_progress_validates_payload: null, missing, NaN, string fractions hide; negative/above-one fractions clamp; zero remains valid. |
| Open menu updates without moving click targets | Existing model_status_menu_keeps_click_targets_until_closed plus model_status_open_menu_progress_refresh_keeps_actor_identity: stable rows/clocks, new progress and completion at same refresh. |
| Focus behavior and lifecycle regression | Existing row focus and unreadCompletionCount cases remain. |
| Clean-home installation | Existing test_install.sh repeated installation and Rust deployment tests include extension JS/CSS assets. |
