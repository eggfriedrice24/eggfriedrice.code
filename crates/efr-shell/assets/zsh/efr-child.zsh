# efr: the child shell of one call that efr-sbx runs for the auto mode.
#
# Written for efr. efr-sbx starts it as `zsh -f efr-child.zsh DIR`, inside the sandbox
# for a contained call and outside it for an approved exit. DIR holds:
#
#   snapshot.zsh  the trusted shell's functions, aliases and options
#   state.zsh     what earlier contained calls left; the exit child's DIR has none,
#                 because it never runs anything that the sandbox made
#   line          the model's command line
#
# A file that is missing skips that step. The script replays the snapshot and the state,
# notes the exported variables, functions and aliases, evaluates the line, and then
# reports on descriptor 3 what changed, in the records format of efr-sandbox:
#
#   efr-records NUL v1 NUL
#   cwd NUL <dir> NUL
#   export NUL <name> NUL <value> NUL      unset NUL <name> NUL
#   func NUL <name> NUL <body> NUL         unfunc NUL <name> NUL
#   alias NUL <name> NUL <value> NUL       unalias NUL <name> NUL
#   end NUL <status> NUL
#
# The line runs in this shell and can change everything here, these records included.
# efr-sbx treats them as untrusted data: it checks every name and value again, and a
# stream without its end record keeps nothing. The report is written by an EXIT trap,
# so it also comes after `exit N`.

builtin typeset -g _efr_child_dir=${1-}
builtin zmodload zsh/parameter

[[ -n $_efr_child_dir && -r $_efr_child_dir/snapshot.zsh ]] &&
  builtin source -- $_efr_child_dir/snapshot.zsh
[[ -n $_efr_child_dir && -r $_efr_child_dir/state.zsh ]] &&
  builtin source -- $_efr_child_dir/state.zsh

# Names that change in every shell or that efr sets; they are never reported.
builtin typeset -ga _efr_child_skip
_efr_child_skip=(PWD OLDPWD SHLVL _ EFR_SANDBOX)

# The exported scalars as name value pairs.
_efr_child_exports() {
  builtin emulate -L zsh -o extended_glob
  builtin local name
  reply=()
  for name in ${(k)parameters[(R)*export*]}; do
    [[ $parameters[$name] == *(array|association)* ]] && continue
    (( ${_efr_child_skip[(Ie)$name]} )) && continue
    reply+=("$name" "${(P)name}")
  done
}

# What the line starts with, as name value lists. A function, so that the user's
# options from the snapshot (KSH_ARRAYS, say) do not change how the lists are read.
builtin typeset -ga _efr_child_env0 _efr_child_fn0 _efr_child_al0
_efr_child_start() {
  builtin emulate -L zsh
  _efr_child_exports
  _efr_child_env0=("${(@)reply}")
  _efr_child_fn0=("${(@kv)functions}")
  _efr_child_al0=("${(@kv)aliases}")
}
_efr_child_start

# Writes the records of the call on descriptor 3; nothing when it is not open.
_efr_child_records() {
  builtin local -i st=$1
  builtin emulate -L zsh -o extended_glob
  builtin local -A was now
  builtin local -a out list
  builtin local name
  out=(efr-records v1 cwd "$PWD")
  # NOTE: each list is first compared whole, which costs one pass; only a list that
  # changed is compared name by name. A line seldom changes one, and a big rc has
  # hundreds of functions and aliases.
  _efr_child_exports
  if [[ ${(pj:\0:)reply} != ${(pj:\0:)_efr_child_env0} ]]; then
    was=("${(@)_efr_child_env0}")
    now=("${(@)reply}")
    for name in ${(k)now}; do
      [[ ${+was[$name]} == 1 && $was[$name] == "$now[$name]" ]] ||
        out+=(export "$name" "$now[$name]")
    done
    for name in ${(k)was}; do
      (( ${+now[$name]} )) || out+=(unset "$name")
    done
  fi
  list=("${(@kv)functions}")
  if [[ ${(pj:\0:)list} != ${(pj:\0:)_efr_child_fn0} ]]; then
    was=("${(@)_efr_child_fn0}")
    now=("${(@)list}")
    for name in ${(k)now:#_efr_*}; do
      [[ ${+was[$name]} == 1 && $was[$name] == "$now[$name]" ]] ||
        out+=(func "$name" "$now[$name]")
    done
    for name in ${(k)was:#_efr_*}; do
      (( ${+now[$name]} )) || out+=(unfunc "$name")
    done
  fi
  list=("${(@kv)aliases}")
  if [[ ${(pj:\0:)list} != ${(pj:\0:)_efr_child_al0} ]]; then
    was=("${(@)_efr_child_al0}")
    now=("${(@)list}")
    for name in ${(k)now}; do
      [[ ${+was[$name]} == 1 && $was[$name] == "$now[$name]" ]] ||
        out+=(alias "$name" "$now[$name]")
    done
    for name in ${(k)was}; do
      (( ${+now[$name]} )) || out+=(unalias "$name")
    done
  fi
  out+=(end "$st")
  { builtin print -rN -- "${(@)out}" } 2>/dev/null >&3
  return 0
}

builtin trap '_efr_child_records $?' EXIT

# The line runs at the top level, as it would at a prompt, with the user's options
# from the snapshot.
if [[ -n $_efr_child_dir && -r $_efr_child_dir/line ]]; then
  builtin eval -- "$(<$_efr_child_dir/line)"
fi
