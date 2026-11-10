_wide() {
  local words cur prev
  words=("${COMP_WORDS[@]}")
  cur=${COMP_WORDS[COMP_CWORD]}
  prev=${COMP_WORDS[COMP_CWORD-1]}
  case "$prev" in
    wide) COMPREPLY=( $(compgen -W 'status advanced --help --version' -- "$cur") ) ;;
    advanced) COMPREPLY=( $(compgen -W 'plan apply sound reconnect modes list info capabilities ddc diagnose mode preset --json --display' -- "$cur") ) ;;
    reconnect) COMPREPLY=( $(compgen -W '--layout --cycle --confirm --json' -- "$cur") ) ;;
    sound) COMPREPLY=( $(compgen -W '--confirm --json' -- "$cur") ) ;;
    plan|apply) COMPREPLY=( $(compgen -W 'fullscreen split-left split-right pip-big pip-small' -- "$cur") ) ;;
    ddc) COMPREPLY=( $(compgen -W 'read write scan' -- "$cur") ) ;;
    scan) COMPREPLY=( $(compgen -W '--read-only --all' -- "$cur") ) ;;
    diagnose) COMPREPLY=( $(compgen -W '--output --label' -- "$cur") ) ;;
  esac
}
complete -F _wide wide
