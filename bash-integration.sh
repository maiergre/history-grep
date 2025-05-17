###### history-grep #####
# Helper function for bash and readline integration. Use
# with bash's `bind -x`
function __history_grep_readline() {
    local tmpfile
    local cmd
    tmpfile=$(mktemp)
    hgr --bash-readline-mode ${tmpfile}
    # Note: using $(<tmpfile) breaks bash 
    cmd=$(cat ${tmpfile})
    if [ -n "${cmd}" ]; then
        READLINE_LINE=${cmd}
        READLINE_POINT=${#cmd}
    fi
}

# Make Crtl-R use `hgr` for searching history entries. 
bind -m emacs-standard -x '"\C-r": __history_grep_readline'
bind -m vi-command -x '"\C-r": __history_grep_readline'
bind -m vi-insert -x '"\C-r": __history_grep_readline'
###### history-grep #####
