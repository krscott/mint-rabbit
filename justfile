set positional-arguments

default:
    just --list

build:
    cargo build --locked

# Run the app. With --decomp PATH, or MINT_AC_DECOMP, first extract that decomp's assets from its disc image.
run *args:
    #!/usr/bin/env bash
    set -euo pipefail
    decomp="${MINT_AC_DECOMP:-}"
    version=GAFE01_00
    args=("$@")
    for ((i = 0; i + 1 < ${#args[@]}; i++)); do
        case "${args[i]}" in
            --decomp) decomp="${args[i + 1]}" ;;
            --game-version) version="${args[i + 1]}" ;;
        esac
    done
    if [[ -n "$decomp" ]]; then
        if [[ ! -f "$decomp/build.ninja" ]]; then
            (cd -- "$decomp" && python3 configure.py --version "$version")
        fi
        ninja -C "$decomp" pre-compile
    fi
    cargo run --locked -p mint-rabbit -- "$@"

test:
    cargo test --locked --workspace

# Parse every model source and the villager table of an ac-decomp checkout (built or not; default $MINT_AC_DECOMP).
test-decomp-sources decomp=env("MINT_AC_DECOMP", ""):
    MINT_AC_DECOMP="$(realpath -- "${1:?pass an ac-decomp checkout, or set MINT_AC_DECOMP to one}")" && export MINT_AC_DECOMP && cargo test --locked -p mint-ac -- --ignored all_model_sources_interpret villager_table_and_pose_parse

# Run every decomp test, including the round trip of every villager; needs a built checkout (default $MINT_AC_DECOMP).
test-decomp decomp=env("MINT_AC_DECOMP", ""):
    MINT_AC_DECOMP="$(realpath -- "${1:?pass an ac-decomp checkout, or set MINT_AC_DECOMP to one}")" && export MINT_AC_DECOMP && cargo test --locked -p mint-ac -- --include-ignored

# Write a generated demo villager as a decomp-shaped directory and open it.
demo dir="target/demo-decomp":
    cargo run --locked -q -p mint-ac --example demo_decomp -- "$1"
    cargo run --locked -p mint-rabbit -- --decomp "$1"

# Render the first villager of DECOMP (default: $MINT_AC_DECOMP, else the demo) to target/model-view.png; needs a GPU.
snapshot decomp=env("MINT_AC_DECOMP", ""):
    #!/usr/bin/env bash
    set -euo pipefail
    decomp="$1"
    if [[ -z "$decomp" ]]; then
        decomp=target/demo-decomp
        cargo run --locked -q -p mint-ac --example demo_decomp -- "$decomp"
    fi
    MINT_AC_DECOMP="$(realpath -- "$decomp")"
    MINT_SNAPSHOT="$PWD/target/model-view.png"
    export MINT_AC_DECOMP MINT_SNAPSHOT
    cargo test --locked -p mint-rabbit -- --ignored model_view_snapshot

clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings

# Format files, or all tracked files when no files are provided.
format *files:
    @just _format write "$@"

# Check formatting without changing files.
format-check *files:
    @just _format check "$@"

[private]
_format mode *files:
    #!/usr/bin/env bash
    set -euo pipefail

    declare -a files=()
    mode="$1"
    shift

    if (($# == 0)); then
        mapfile -d '' -t files < <(git ls-files -z)
    else
        files=("$@")
    fi

    for file in "${files[@]}"; do
        if [ ! -f "$file" ]; then
            continue
        fi

        case "$file" in
            justfile|Justfile|.justfile|*/justfile|*/Justfile|*/.justfile)
                if [[ "$mode" == check ]]; then
                    just --unstable --fmt --check --justfile "$file"
                else
                    just --unstable --fmt --justfile "$file"
                fi
            ;;
            *.nix)
                if [[ "$mode" == check ]]; then
                    nixfmt --check "$file"
                else
                    nixfmt "$file"
                fi
            ;;
            *.rs)
                if [[ "$mode" == check ]]; then
                    rustfmt --edition 2024 --check "$file"
                else
                    rustfmt --edition 2024 "$file"
                fi
            ;;
            *.sh|.githooks/*|*/.githooks/*)
                if [[ "$mode" == check ]]; then
                    shfmt -d -i 4 "$file"
                else
                    shfmt -w -i 4 "$file"
                fi
            ;;
        esac
    done

# Check tracked shell scripts and Git hooks.
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    while IFS= read -r -d '' file; do
        [[ -f "$file" ]] || continue
        case "$file" in
            *.sh|.githooks/*) shellcheck "$file" ;;
        esac
    done < <(git ls-files -z)

# Run non-mutating formatting, shell, and Nix checks.
check: format-check lint clippy
    nix flake check -L --no-update-lock-file

# Format staged files for git pre-commit.
pre-commit:
    #!/usr/bin/env bash
    set -euo pipefail

    declare -a staged_files
    declare -a format_files=()
    declare -A partially_staged
    file=''

    mapfile -d '' -t staged_files < <(
        git diff --cached --name-only --diff-filter=ACMR -z
    )

    while IFS= read -r -d '' file; do
        partially_staged["$file"]=1
    done < <(git diff --name-only -z)

    for file in "${staged_files[@]}"; do
        if [[ -v partially_staged["$file"] ]]; then
            continue
        fi

        format_files+=("$file")
    done

    if ((${#format_files[@]} == 0)); then
        exit 0
    fi

    just format "${format_files[@]}"
    git add -- "${format_files[@]}"

# Install a pre-commit hook that formats git-changed files.
install-hooks:
    git config core.hooksPath .githooks
