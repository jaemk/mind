export const meta = {
  name: '{{ns:hello}}',
  description: 'Greet the user and summarize the repo state',
  whenToUse: 'when a session starts and you want a quick orientation',
  phases: [{ title: 'Greet' }],
}

phase('Greet')
const status = await agent('Summarize the current git status and recent commits in one line.')
log(`hello: ${status}`)
