#!/bin/sh
set -eu

: "${GH_TOKEN:?set GH_TOKEN}"
: "${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY}"

command -v gh >/dev/null
command -v jq >/dev/null
command -v curl >/dev/null

validate_name() {
    name=$1
    case "$name" in
        ''|.|..|*/*|*\\*)
            printf 'invalid release asset name: %s\n' "$name" >&2
            exit 1
            ;;
    esac
}

upload_file() {
    release_id=$1
    file=$2
    test -f "$file"
    name=$(basename "$file")
    validate_name "$name"
    encoded=$(jq -rn --arg value "$name" '$value|@uri')
    curl --fail --silent --show-error \
        --request POST \
        --header "Authorization: Bearer $GH_TOKEN" \
        --header 'X-GitHub-Api-Version: 2022-11-28' \
        --header 'Content-Type: application/octet-stream' \
        --data-binary "@$file" \
        "https://uploads.github.com/repos/$GITHUB_REPOSITORY/releases/$release_id/assets?name=$encoded" \
        >/dev/null
}

download_all() {
    release_id=$1
    output=$2
    rm -rf "$output"
    mkdir -p "$output"
    listing=$(mktemp)
    gh api --paginate "repos/$GITHUB_REPOSITORY/releases/$release_id/assets?per_page=100" \
        | jq -s 'add // []' > "$listing"
    jq -r '.[] | [.id,.name] | @tsv' "$listing" | while IFS="	" read -r id name
    do
        validate_name "$name"
        gh api -H 'Accept: application/octet-stream' \
            "repos/$GITHUB_REPOSITORY/releases/assets/$id" > "$output/$name"
    done
    rm -f "$listing"
}

asset_count() {
    release_id=$1
    gh api --paginate "repos/$GITHUB_REPOSITORY/releases/$release_id/assets?per_page=100" \
        | jq -s 'add // [] | length'
}

case "${1:-}" in
    upload)
        test "$#" -eq 3
        upload_file "$2" "$3"
        ;;
    upload-dir)
        test "$#" -eq 3
        release_id=$2
        directory=$3
        test -d "$directory"
        count=0
        for file in "$directory"/*
        do
            test -f "$file"
            upload_file "$release_id" "$file"
            count=$((count + 1))
        done
        test "$count" -gt 0
        ;;
    download-all)
        test "$#" -eq 3
        download_all "$2" "$3"
        ;;
    count)
        test "$#" -eq 2
        asset_count "$2"
        ;;
    *)
        echo 'usage: github-draft-assets.sh upload RELEASE_ID FILE | upload-dir RELEASE_ID DIR | download-all RELEASE_ID DIR | count RELEASE_ID' >&2
        exit 2
        ;;
esac
