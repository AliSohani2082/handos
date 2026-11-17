#!/usr/bin/env bash
# Spike 3 text test: an English and a Persian sentence into a terminal, a browser field, a GTK
# application and a Qt application, by generated keymap and by paste. Each app is launched,
# the typer refuses unless that app's window is active, and the received text is compared.
#
#   [APPS="terminal qt gtk browser"] [METHODS="keymap paste"] [KEY_DELAY=2] spikes/input/harness/text_matrix.sh [results.jsonl]
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
SPIKE="$HERE/../../target/release/input-spike"
OUTFILE=${1:-$HERE/../../results/input-text.jsonl}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
: > "$OUTFILE"

EN='The quick brown fox jumps over the lazy dog, 42 times!'
FA='سلام دنیا، من می‌خواهم این متن را بنویسم.'

wait_class() { # pattern timeout_s
    local end=$((SECONDS + $2))
    while (( SECONDS < end )); do
        hyprctl activewindow -j 2>/dev/null | grep -qi "\"class\": \"[^\"]*$1" && return 0
        sleep 0.2
    done
    return 1
}

launch() { # app out_file -> sets PID, CLASS, PASTE
    case $1 in
        terminal) CLASS=handos-spike-term; PASTE=ctrl-shift-v
            kitty --class "$CLASS" -e sh -c 'IFS= read -r l; printf "%s" "$l" > "$1"' sh "$2" >/dev/null 2>&1 & PID=$! ;;
        qt) CLASS=kdialog; PASTE=ctrl-v
            sh -c 'kdialog --title "handOs spike" --inputbox "type here" > "$1"' sh "$2" 2>/dev/null & PID=$! ;;
        gtk) CLASS=handos; PASTE=ctrl-v
            /usr/bin/python3 "$HERE/gtk_entry.py" "$2" >/dev/null 2>&1 & PID=$! ;;
        browser) CLASS=chrom; PASTE=ctrl-v
            local port=$((20000 + RANDOM % 20000))
            /usr/bin/python3 "$HERE/form_server.py" "$port" "$2" & SERVER=$!
            chromium --user-data-dir="$WORK/chromium" --no-first-run --no-default-browser-check \
                --app="http://127.0.0.1:$port/" >/dev/null 2>&1 & PID=$! ;;
    esac
}

for app in ${APPS:-terminal qt gtk browser}; do
    for method in ${METHODS:-keymap paste}; do
        for lang in en fa; do
            text=$EN; [[ $lang == fa ]] && text=$FA
            out="$WORK/$app-$method-$lang.txt"; rm -f "$out"; SERVER=
            launch "$app" "$out"
            if ! wait_class "$CLASS" 15; then
                result="app window never became active"; ok=false
            else
                sleep 1.0
                "$SPIKE" type --method "$method" --text "$text" --enter --paste-keys "$PASTE" --expect-class "$CLASS" --key-delay-ms "${KEY_DELAY:-2}" >/dev/null
                rc=$?
                for _ in $(seq 50); do [[ -s $out ]] && break; sleep 0.1; done
                got=$(cat "$out" 2>/dev/null; printf x); got=${got%x}
                got=${got%$'\n'}
                if (( rc != 0 )); then result="typer refused or failed (exit $rc)"; ok=false
                elif [[ $got == "$text" ]]; then result=exact; ok=true
                else result="got: $got"; ok=false
                fi
            fi
            kill "$PID" 2>/dev/null; [[ -n $SERVER ]] && kill "$SERVER" 2>/dev/null
            [[ $app == browser ]] && pkill -f "user-data-dir=$WORK/chromium" 2>/dev/null
            wait "$PID" 2>/dev/null
            printf '%-9s %-7s %-3s delay=%-3s %-6s %s\n' "$app" "$method" "$lang" "${KEY_DELAY:-2}" "$ok" "$result"
            python3 -c 'import json,sys; print(json.dumps(dict(zip(["app","method","lang","ok","result"], sys.argv[1:4] + [sys.argv[4]=="true", sys.argv[5]])), ensure_ascii=False))' \
                "$app" "$method" "$lang" "$ok" "$result (key delay ${KEY_DELAY:-2} ms)" >> "$OUTFILE"
            sleep 0.5
        done
    done
done
