complete -c wide -f
complete -c wide -n '__fish_use_subcommand' -a 'status advanced'
complete -c wide -n '__fish_seen_subcommand_from advanced' -a 'plan apply sound reconnect modes list info capabilities ddc diagnose mode preset'
complete -c wide -n '__fish_seen_subcommand_from reconnect' -l cycle -d 'Attempt one logical display reconnect'
complete -c wide -n '__fish_seen_subcommand_from reconnect' -l layout -r -a 'fullscreen split-left split-right pip-big pip-small'
complete -c wide -n '__fish_seen_subcommand_from sound' -l confirm -d 'Apply current-layout audio policy only'
complete -c wide -n '__fish_seen_subcommand_from plan apply' -a 'fullscreen split-left split-right pip-big pip-small'
