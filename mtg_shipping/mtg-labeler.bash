_mtg_labeler() {
    local cur opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    opts="--start --test --validate --clean-cache --no-cache --force --pdf --output"
    COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
}
complete -F _mtg_labeler mtg-labeler
