export function secretInputType(revealed: boolean): 'password' | 'text' {
  return revealed ? 'text' : 'password'
}

export function secretToggleText(revealed: boolean): 'Hide' | 'Show' {
  return revealed ? 'Hide' : 'Show'
}

export function secretToggleAriaLabel(revealed: boolean): 'Hide value' | 'Show value' {
  return revealed ? 'Hide value' : 'Show value'
}
