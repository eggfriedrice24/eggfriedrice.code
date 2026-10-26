# Decoder fixtures of the app

These items are not frozen fixtures of efr-protocol. efr-protocol cannot write them,
because each one holds a value that no build of efrd knows yet. They show what a newer
efrd can send, and `generated-zod.test.ts` checks that the decoder skips each one and
goes on.

| File                                   | What it holds                                              | Decodes to                                  |
| -------------------------------------- | ---------------------------------------------------------- | ------------------------------------------- |
| `item-unknown-kind.json`               | A subscribe item of a kind that the protocol does not list | an `other` item                             |
| `item-event-unknown-nested-value.json` | A `tool_call_started` event with a new kind of `launch`    | an event item with an invalid `other` event |
| `item-draft-unknown-nested-value.json` | A `compacting` draft with a new `trigger`                  | a draft item with an invalid `other` part   |
