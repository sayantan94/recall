/// Output the zsh hook script to stdout.
/// User activates with: eval "$(recall init zsh)"
/// or: eval "$(./target/release/recall init zsh)"
///
/// The hook embeds the absolute path to the current binary,
/// so it works regardless of whether recall is in PATH.
pub fn print_hook() {
    let bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "recall".to_string());

    print!(
        r#"__recall_preexec() {{
    export __RECALL_CMD="$1"
    export __RECALL_START=$(python3 -c 'import time;print(int(time.time()*1000))')
}}

__recall_precmd() {{
    local exit_code=$?
    if [ -n "$__RECALL_CMD" ]; then
        "{bin}" log \
            --command "$__RECALL_CMD" \
            --exit-code $exit_code \
            --start "$__RECALL_START" \
            --cwd "$PWD" \
            --session "$RECALL_SESSION_ID" \
            --terminal "${{TERM_PROGRAM:-${{TERMINAL_EMULATOR:-${{LC_TERMINAL:-Terminal}}}}}}" &!
        unset __RECALL_CMD
    fi
}}

export RECALL_SESSION_ID=$("{bin}" session-id)
autoload -Uz add-zsh-hook
add-zsh-hook preexec __recall_preexec
add-zsh-hook precmd __recall_precmd
"#,
        bin = bin
    );
}
