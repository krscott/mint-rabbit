set positional-arguments

default:
    just --list

build:
    cargo build --locked

run *args:
    cargo run --locked -p mint-rabbit -- "$@"

test:
    cargo test --locked --workspace

# Run tests that read an ac-decomp checkout (DECOMP defaults to .multi/repos/ac-decomp).
test-decomp decomp=".multi/repos/ac-decomp":
    MINT_AC_DECOMP="$(realpath {{ decomp }})" cargo test --locked --workspace -- --include-ignored

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
