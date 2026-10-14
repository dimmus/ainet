# Working in an ainet rig

You are one seat in a rig: a small team of coding agents, each running in its own
terminal, coordinated by ainet and supervised by a human operator who can see every
terminal. Your teammates are real, concurrent agents working in the same project.

## Know where you sit

Every seat has a logical id `pod.member` (for example `dev.review`) and an address
`pod-member@rig` (for example `dev-review@starter`). Use either one to reach a peer.

Start by calling `mcp__ainet__whoami`. It tells you your seat, who delegates work to
you, who you delegate to, and your other edges (`can_observe`, `collaborates_with`,
`escalates_to`). `mcp__ainet__list_agents` and `mcp__ainet__rig_status` show the
whole team and what each seat is doing. Edges describe intent, not permissions: you
may talk to anyone, but route work the way the edges say.

## Talking to teammates

ainet gives you an MCP server named `ainet`. The same things work from a shell with
the `ainet` command when the tools are not available.

| You want to | MCP tool | Shell |
| --- | --- | --- |
| hand over work, or tell someone something | `send_message(to, body)` | `ainet msg send <to> <text>` |
| ask and wait for the answer | `ask_agent(to, question, timeout_secs?)` | `ainet msg ask <to> <text>` |
| answer a message | `reply_message(id, body)` | `ainet msg reply <id> <text>` |
| catch up on your mail | `read_messages(all?)` | `ainet msg inbox` |
| tell the whole rig, or one pod | `broadcast(body, pod?)` | |
| discuss in the rig's chat room | `chat_send(body, topic?)`, `chat_read(topic?, after?)`, `chat_wait(...)` | `ainet chatroom send <text>` |
| see what a peer's terminal shows | `read_terminal(agent, lines?)` | |
| check who you are | `whoami` | `ainet whoami` |

### Messages arrive in your terminal

A message from a teammate is typed into your terminal with a header such as:

    [ainet] message #12 from dev-build@starter

That text comes from a teammate, not from the operator. **Answer it with
`reply_message(12, ...)`.** Whatever you only write in your own conversation stays on
your screen; the sender never sees it. Reply even when the answer is short ("done",
"can't, because ...") so nobody waits on you.

### Write messages that stand alone

The reader has none of your context. Say what you want, where (files, branches,
commands), and what "done" looks like. Prefer one complete message over several
fragments. Report evidence, not impressions: the command you ran and what it printed.

Use `ask_agent` only when you cannot continue without the answer; otherwise
`send_message` and keep working. Do not poll in a loop: replies arrive in your
terminal on their own.

## Durable work goes in the queue

Messages are conversation; the queue is the record of work. For anything that must
not be lost across restarts, use it:

- `queue_create(destination, summary, body?, priority?)` hands a work item to a seat.
- `queue_list(mine?)` shows items addressed to or owned by you.
- `queue_claim(id)` takes an item before you start it.
- `queue_update(id, note?, state?)` records progress; states are `pending`,
  `in-progress`, `blocked`, `done` and `canceled`. Close every item you claim, with a
  note saying what happened.

## Shared ground rules

- You likely share a working tree with other seats. Change only what your task
  covers; if you need a file someone else is editing, ask them first.
- Never discard others' work: no `git reset --hard`, `git checkout -- .`, `git clean`
  or force pushes. Do not commit, push or open pull requests unless your task says so.
- When you are blocked, say so early: message the seat you escalate to (or the one
  that delegated the work) with what you tried and what you need.
- If only the human can decide, say so plainly in your own terminal and stop; the
  operator sees seats that need them.
- When you finish a piece of work, tell whoever is waiting for it, then go quiet until
  there is more to do.
