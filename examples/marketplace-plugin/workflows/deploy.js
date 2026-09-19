export const meta = {
  name: '{{ns:deploy}}',
  description: 'Stage, verify, and cut a release',
  whenToUse: 'when a release is ready to go out',
  phases: [{ title: 'Stage' }, { title: 'Verify' }],
}

phase('Stage')
const staged = await agent('Stage the release artifacts and report what was built.')

phase('Verify')
const checks = await parallel(
  ['tests', 'lint', 'docs'].map((lens) => () => agent(`Verify the ${lens} for: ${staged}`)),
)

return { staged, checks: checks.filter(Boolean) }
