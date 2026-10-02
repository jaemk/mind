export const meta = {
  name: '{{ns:ship-it}}',
  description: 'Run the release checks, then push the tag',
  phases: [{ title: 'Check' }],
}

phase('Check')
const checks = await parallel(
  ['tests', 'changelog'].map((lens) => () => agent(`Check the ${lens} before shipping.`)),
)

return checks.filter(Boolean)
