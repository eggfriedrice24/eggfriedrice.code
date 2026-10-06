# The child shell of the launcher's tests: the contract that efr-shell's
# assets/zsh/efr-child.zsh keeps (the spec's sections 6.2 and 6.4).
#
#   zsh -f efr-child.zsh DIR
#
# DIR holds snapshot.zsh, state.zsh (contained calls only) and line. The script replays
# the snapshot and the state, evaluates the line, and an EXIT trap writes the records on
# fd 3: the final cwd, the exports, unsets, functions and aliases that changed, and the
# status. Names that start with _efr_ are skipped.

builtin zmodload zsh/parameter
typeset -g _efr_child_dir=$1
[[ -r $_efr_child_dir/snapshot.zsh ]] && builtin source $_efr_child_dir/snapshot.zsh
[[ -r $_efr_child_dir/state.zsh ]] && builtin source $_efr_child_dir/state.zsh

typeset -gA _efr_child_env0 _efr_child_fn0 _efr_child_al0
() {
  local n
  for n in ${(k)parameters[(R)*export*]}; do _efr_child_env0[$n]=${(P)n}; done
  for n in ${(k)functions}; do _efr_child_fn0[$n]=$functions[$n]; done
  for n in ${(k)aliases}; do _efr_child_al0[$n]=$aliases[$n]; done
}

_efr_child_records() {
  local rc=$1 out n v
  out=$'efr-records\0v1\0'
  out+="cwd"$'\0'"$PWD"$'\0'
  for n in ${(k)parameters[(R)*export*]}; do
    [[ $n == _efr_* ]] && continue
    v=${(P)n}
    if (( ! ${+_efr_child_env0[$n]} )) || [[ $_efr_child_env0[$n] != $v ]]; then
      out+="export"$'\0'"$n"$'\0'"$v"$'\0'
    fi
  done
  for n in ${(k)_efr_child_env0}; do
    [[ ${parameters[$n]-} == *export* ]] || out+="unset"$'\0'"$n"$'\0'
  done
  for n in ${(k)functions}; do
    [[ $n == _efr_* ]] && continue
    if (( ! ${+_efr_child_fn0[$n]} )) || [[ $_efr_child_fn0[$n] != $functions[$n] ]]; then
      out+="func"$'\0'"$n"$'\0'"$functions[$n]"$'\0'
    fi
  done
  for n in ${(k)_efr_child_fn0}; do
    (( ${+functions[$n]} )) || out+="unfunc"$'\0'"$n"$'\0'
  done
  for n in ${(k)aliases}; do
    if (( ! ${+_efr_child_al0[$n]} )) || [[ $_efr_child_al0[$n] != $aliases[$n] ]]; then
      out+="alias"$'\0'"$n"$'\0'"$aliases[$n]"$'\0'
    fi
  done
  for n in ${(k)_efr_child_al0}; do
    (( ${+aliases[$n]} )) || out+="unalias"$'\0'"$n"$'\0'
  done
  out+="end"$'\0'"$rc"$'\0'
  builtin print -rn -- "$out" >&3 2>/dev/null
}

trap '_efr_child_records $?' EXIT
builtin eval "$(<$_efr_child_dir/line)"
