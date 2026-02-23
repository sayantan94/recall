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
    export __RECALL_OUTPUT_FILE=$(mktemp /tmp/recall_output.XXXXXX)
    # Record current position in the script typescript file
    if [[ -n "$__RECALL_TYPESCRIPT" && -f "$__RECALL_TYPESCRIPT" ]]; then
        export __RECALL_TS_POS=$(command wc -c < "$__RECALL_TYPESCRIPT" | tr -d ' ')
    fi
}}

__recall_precmd() {{
    local exit_code=$?
    if [ -n "$__RECALL_CMD" ]; then
        # Extract this command's output from the typescript file
        if [[ -n "$__RECALL_TYPESCRIPT" && -n "$__RECALL_TS_POS" && -f "$__RECALL_TYPESCRIPT" ]]; then
            tail -c +$((__RECALL_TS_POS + 1)) "$__RECALL_TYPESCRIPT" > "$__RECALL_OUTPUT_FILE" 2>/dev/null
        fi
        "{bin}" log \
            --command "$__RECALL_CMD" \
            --exit-code $exit_code \
            --start "$__RECALL_START" \
            --cwd "$PWD" \
            --session "$RECALL_SESSION_ID" \
            --terminal "${{TERM_PROGRAM:-${{TERMINAL_EMULATOR:-${{LC_TERMINAL:-Terminal}}}}}}" \
            --output-file "$__RECALL_OUTPUT_FILE" &!
        unset __RECALL_CMD
    fi
}}

export RECALL_SESSION_ID=$("{bin}" session-id)
autoload -Uz add-zsh-hook
add-zsh-hook preexec __recall_preexec
add-zsh-hook precmd __recall_precmd

# Start a script session to capture output through a PTY (preserves colors)
if [[ -z "$__RECALL_TYPESCRIPT" ]]; then
    export __RECALL_TYPESCRIPT=$(mktemp /tmp/recall_typescript.XXXXXX)
    SHELL=$(command -v zsh) exec script -q "$__RECALL_TYPESCRIPT"
fi
"#,
        bin = bin
    );
}
