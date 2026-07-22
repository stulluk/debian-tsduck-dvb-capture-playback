_dvbhub() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="dvbhub"
                ;;
            dvbhub,analyze)
                cmd="dvbhub__subcmd__analyze"
                ;;
            dvbhub,capture)
                cmd="dvbhub__subcmd__capture"
                ;;
            dvbhub,capture-all)
                cmd="dvbhub__subcmd__capture__subcmd__all"
                ;;
            dvbhub,completions)
                cmd="dvbhub__subcmd__completions"
                ;;
            dvbhub,devices)
                cmd="dvbhub__subcmd__devices"
                ;;
            dvbhub,help)
                cmd="dvbhub__subcmd__help"
                ;;
            dvbhub,loopback)
                cmd="dvbhub__subcmd__loopback"
                ;;
            dvbhub,play)
                cmd="dvbhub__subcmd__playout"
                ;;
            dvbhub,playout)
                cmd="dvbhub__subcmd__playout"
                ;;
            dvbhub,serve)
                cmd="dvbhub__subcmd__serve"
                ;;
            dvbhub,transponders)
                cmd="dvbhub__subcmd__transponders"
                ;;
            dvbhub,tsduck)
                cmd="dvbhub__subcmd__tsduck"
                ;;
            dvbhub,update-satellites-json)
                cmd="dvbhub__subcmd__update__subcmd__satellites__subcmd__json"
                ;;
            dvbhub,watch)
                cmd="dvbhub__subcmd__watch"
                ;;
            dvbhub__subcmd__help,analyze)
                cmd="dvbhub__subcmd__help__subcmd__analyze"
                ;;
            dvbhub__subcmd__help,capture)
                cmd="dvbhub__subcmd__help__subcmd__capture"
                ;;
            dvbhub__subcmd__help,capture-all)
                cmd="dvbhub__subcmd__help__subcmd__capture__subcmd__all"
                ;;
            dvbhub__subcmd__help,completions)
                cmd="dvbhub__subcmd__help__subcmd__completions"
                ;;
            dvbhub__subcmd__help,devices)
                cmd="dvbhub__subcmd__help__subcmd__devices"
                ;;
            dvbhub__subcmd__help,help)
                cmd="dvbhub__subcmd__help__subcmd__help"
                ;;
            dvbhub__subcmd__help,loopback)
                cmd="dvbhub__subcmd__help__subcmd__loopback"
                ;;
            dvbhub__subcmd__help,playout)
                cmd="dvbhub__subcmd__help__subcmd__playout"
                ;;
            dvbhub__subcmd__help,serve)
                cmd="dvbhub__subcmd__help__subcmd__serve"
                ;;
            dvbhub__subcmd__help,transponders)
                cmd="dvbhub__subcmd__help__subcmd__transponders"
                ;;
            dvbhub__subcmd__help,tsduck)
                cmd="dvbhub__subcmd__help__subcmd__tsduck"
                ;;
            dvbhub__subcmd__help,update-satellites-json)
                cmd="dvbhub__subcmd__help__subcmd__update__subcmd__satellites__subcmd__json"
                ;;
            dvbhub__subcmd__help,watch)
                cmd="dvbhub__subcmd__help__subcmd__watch"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        dvbhub)
            opts="-h -V --help --version devices transponders tsduck capture capture-all playout play update-satellites-json loopback analyze watch serve completions help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__analyze)
            opts="-h --input --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --input)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__capture)
            opts="-h --transponder --device-name --adapter --delivery-system --frequency-mhz --polarity --symbol-rate-ksps --modulation --fec --lnb --satellite-number --signal-timeout --seconds --output --tui --tui-auto --satellite --full-satellite --output-dir --max-tps --resume --tp-frequency-mhz --tp-polarity --tp-symbol-rate-ksps --analyze --dry-run --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --transponder)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --device-name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --adapter)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delivery-system)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --frequency-mhz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --polarity)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --symbol-rate-ksps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --modulation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --fec)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lnb)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --satellite-number)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --signal-timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --seconds)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --satellite)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --max-tps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tp-frequency-mhz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tp-polarity)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tp-symbol-rate-ksps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__capture__subcmd__all)
            opts="-h --device-name --adapter --output-dir --seconds --signal-timeout --satellite-number --keep-going --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --device-name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --adapter)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --seconds)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --signal-timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --satellite-number)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__completions)
            opts="-h --help bash elvish fish powershell zsh"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__devices)
            opts="-h --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help)
            opts="devices transponders tsduck capture capture-all playout update-satellites-json loopback analyze watch serve completions help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__analyze)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__capture)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__capture__subcmd__all)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__completions)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__devices)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__loopback)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__playout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__serve)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__transponders)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__tsduck)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__update__subcmd__satellites__subcmd__json)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__help__subcmd__watch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__loopback)
            opts="-h --input --if-mhz --modulation --symbol-rate-ksps --code-rate --tuner-device --warmup --seconds --output --dry-run --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --input)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --if-mhz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --modulation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --symbol-rate-ksps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --code-rate)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tuner-device)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --warmup)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --seconds)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__playout)
            opts="-h --input --frequency-mhz --modulation --symbol-rate-ksps --code-rate --infinite --level --dry-run --tui --tui-auto --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --input)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --frequency-mhz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --modulation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --symbol-rate-ksps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --code-rate)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --level)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__serve)
            opts="-h --bind --port --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --bind)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --port)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__transponders)
            opts="-h --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__tsduck)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__update__subcmd__satellites__subcmd__json)
            opts="-h --satellite --dry-run --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --satellite)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        dvbhub__subcmd__watch)
            opts="-h --transponder --device-name --adapter --service --delivery-system --frequency-mhz --polarity --symbol-rate-ksps --modulation --fec --lnb --satellite-number --signal-timeout --player --transcode-audio --dry-run --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --transponder)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --device-name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --adapter)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --service)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --delivery-system)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --frequency-mhz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --polarity)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --symbol-rate-ksps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --modulation)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --fec)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lnb)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --satellite-number)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --signal-timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --player)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _dvbhub -o nosort -o bashdefault -o default dvbhub
else
    complete -F _dvbhub -o bashdefault -o default dvbhub
fi
