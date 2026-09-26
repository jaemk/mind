export const meta = {
  name: 'release',
  description: 'Cut a release from the declared workflows path',
  whenToUse: 'when the plugin manifest points workflows elsewhere',
  phases: [{ title: 'Cut' }],
}

phase('Cut')
const tag = await agent('Compute the next release tag and create it.')
return { tag }
