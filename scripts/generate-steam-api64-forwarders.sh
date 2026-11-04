#!/usr/bin/env bash
# Read original PE export table. Refuse ordinal-only or invalid exports.
set -euo pipefail
[[ $# == 2 && "$1" = /* && "$2" = /* && -f "$1" && ! -e "$2" ]] || {
  echo "Expected absolute ORIGINAL_DLL and nonexistent absolute OUTPUT_DEF" >&2; exit 2;
}
temp="$(mktemp "$2.tmp.XXXXXX")"
trap 'rm -f -- "$temp"' EXIT
"$(printenv INTIMATR_OBJDUMP || printf objdump)" -p "$1" | awk '
function fromhex(s, i,n,d) {
  s=tolower(s); sub(/^0x/,"",s); n=0
  for (i=1;i<=length(s);i++) {
    d=index("0123456789abcdef",substr(s,i,1))-1
    if(d<0)return -1
    n=n*16+d
  }
  return n
}
BEGIN {print "LIBRARY steam_api64.dll";print "EXPORTS"}
# GNU objdump uses a two-column "Number in:" display, not the
# NumberOfFunctions/NumberOfNames keys found in some PE readers.
$1=="NumberOfFunctions"{functions=$2}
$1=="NumberOfNames"{names=$2}
$1=="Export" && $2=="Address" && $3=="Table" && $4 ~ /^[0-9A-Fa-f]+$/{functions=$4}
$1=="[Name" && $2=="Pointer/Ordinal]" && $3=="Table" && $4 ~ /^[0-9A-Fa-f]+$/{names=$4}
/\[Ordinal\/Name Pointer\] Table/{in_table=1;next}
in_table && /\+base\[[[:space:]]*[0-9]+\]/{
  symbol=$NF; ordinal=$0
  sub(/^.*\+base\[/,"",ordinal);sub(/\].*$/,"",ordinal)
  gsub(/[[:space:]]/,"",ordinal)
  if(symbol !~ /^[A-Za-z_?$@][A-Za-z0-9_?$@]*$/ || ordinal !~ /^[0-9]+$/ || seen[symbol]++){bad=1;next}
  count++
  printf "    %s=steam_api64_intimatr_original.%s @%d\n",symbol,symbol,ordinal+0
}
END{
  if(bad || functions=="" || names=="" || fromhex(functions)<=0 ||
    fromhex(functions)!=fromhex(names) || count!=fromhex(names)){
      print "Refusing incomplete or ordinal-only export table" > "/dev/stderr";exit 1
  }
}' >"$temp"
mv -- "$temp" "$2"
echo "Generated $(grep -c '=steam_api64_intimatr_original\.' "$2") safe forwarders"
