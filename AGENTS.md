# Mandatory rules.

You HAVE to follow all these rules by heart as it is MANDATORY and REQUIRED, not following them will have FAILED your mission entirely.

## Effiency

If you have to run tests, run scopped one using nextest. Never the full suite unless the entire given task is genuinely finished.

Never spend time deliberating, do not spend 10 tool calls deliberating. Be extremly efficient and implement all of the work given, NEVER deffer anything under THIS rule.

I really DO NOT like when you use cargo tests/check/nextest/build all the time, instead what I prefer when you build a lot then AFTER finshing building you check.

Fixing a test is EASY and FAST. Running cargo tests or build is SLOW and INNEFICIENT. So treat them as an scarse expensive ressource you do not use unless truly needed, every 10 tool calls is something you can use anytime. Tests aren't and they take minutes, so to make your work efficient you HAVE to not run tests all the time.

ALWAYS avoid making pointless tests. Tests have to be earned not something by default for every small thing, but tests should be used to ensure something cannot break.

# Sub Agents

Never use sub agents. Always be extremly efficient in tool calls.

# Code Quality and rules

Avoid comments in code when explaining what, not why. In most cases they aren't necessary so usually avoid comments.

# Language around Code and User.

No purple prose. No ornate language. Write like you're talking. Write plainly. Never in a claude way.
If a five-word sentence works, don't write a thirty-word one. Never prioritize sounding smart over being understood.

All this work is public, stay professional like an agent would as if upstreaming work to rust or llvm. !Important

## GIT

Commit without asking. NEVER use branches. Stay on main. ALWAYS ensure the commits are signed, don't contain any Claude or harness URL/message as our commit space isn't an ad space.

## Roadmap and Handoff

The roadmap is at roadmap/. The important document is KLF-RIDL.md in it. It is extremly large so don't read the entirety of it.
What is implemented is in IMPLEMENTED.md in it.

## File size rule

Code files never exceeds 700 lines in this repo, if you make or see one, fix this, do not deffer this.

# Specs

Any spec is the rule. Read it before changing what it covers and do exactly what it says. If the spec does not say it, it does not exist: never invent a layout, encoding, convention or meaning to make a case work, and never route around a limit the spec sets. A case the spec does not cover is a question for the user, not a decision.
