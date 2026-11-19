# 0028: Push notifications through the Expo push service

Status: accepted, 2026-11-19.

## Context

The phone app shows a conversation only while it is open. A question of the model or
an approval waits for the user, and the user does not know it while the phone is in a
pocket. A push notification tells the user.

An iPhone gets a push only from Apple (APNs). A sender needs the APNs key of the app.
The first design of 0.0.13 sent each push from efrd to APNs, with a `.p8` key that the
user imported into efrd. The user's APNs key is at EAS (Expo), as for the user's other
apps, so no key file is on the machine, and the user wants no new key to keep.

ADR 0022 rejected a hosted relay: a third party and an account that carry the session
(the prompts, the output, the approvals), for work that WireGuard already does.

## Decision

1. **The path.** efrd sends a JSON request to the Expo push service
   (`POST https://exp.host/--/api/v2/push/send`). Expo sends the notice to Apple with
   the APNs key that EAS holds for the app, and Apple sends it to the phone. efrd keeps
   no key, signs nothing and has no secret for push. It uses the shared HTTP client of
   efr-http through a typed client, `efr_http::ExpoPush`, and sends no access token.

2. **When efrd pushes.** All of these must be true: something needs the user (an
   approval or a question of the model, a command that waits for input, the end, the
   fail or the wake limit of a job notice turn, or the end of a turn that the phone
   started and that took at least `notify.after_secs`); `presence.push` is on; the user
   is not at the terminal of that conversation (efr counts the user away, or the
   newest input of the conversation came from a phone); no connection of that phone
   reports a visible lease; and the phone has a token and proved its key in the last
   30 days. One question pushes once.

3. **The body follows the privacy rule of the desktop notice.** The title is `efr`, and
   the body is the place and what happened, such as `app: a question waits`. A push
   never holds a prompt, a command, a title or text of the model. Its data holds only
   the daemon id and the conversation id, which a tap needs.

4. **The third party.** Expo gets each notice (the title, the place and the two ids)
   and the Expo push token of the phone. From the app, Expo gets the APNs token and the
   bundle id of the phone. The Expo docs say that Expo keeps a notice only in memory
   and in queues, not in a database, and that its staff can see the content while they
   debug the service. The service costs nothing, and a send needs no login while the
   access token of the Expo account is off. While Expo is down, no push comes.

5. **Why this does not go against ADR 0022.** The session stays on the tailnet. Expo
   carries only the notice of point 3, efr runs no service, and Expo is the push
   provider that holds the user's APNs key already.

6. **The token.** The app sends its Expo push token with `device.push_set` on each
   connection while its switch is on, and no token to turn push off. efrd checks the
   form `ExponentPushToken[...]` and keeps the token in the table `device_push` of its
   database. A revoke deletes the row in the same transaction, and a
   `DeviceNotRegistered` answer of Expo deletes it when it still holds the token of that
   push. The token is a secret: it is `SecretText` on the wire, it goes in the JSON body
   and not in a URL, the request is not recorded, and no log holds it. efr-http reads
   only the status, the id and the codes of an answer, takes a code only when it is 1
   to 64 characters of `[A-Za-z_]`, and drops each `message` text, which can quote the
   token.

7. **The answers.** efrd keeps each ticket in memory (at most 1,000) and asks for its
   receipt 15 minutes later, with one more try. A request that fails as a whole gets
   up to 4 tries, with waits of 2, 4 and 8 s and a `Retry-After` of up to 60 s, and no
   try after the `ttl` of the notice. Each try and each read of an answer ends after
   10 s on the clock. efrd never sends a notice again after a receipt.

8. **Tests reach a fake.** `EFR_EXPO_PUSH_URL` replaces `https://exp.host`. efrd reads
   it once at its start and writes one warning to its log when it is set. An invalid
   URL turns push off with one warning. The test daemons send to
   `http://127.0.0.1:9`, which answers nothing, unless a test gives its own fake
   server. No test sends to `exp.host`.

## Consequences

- A person who has the Expo token of a phone can send a notice to it, because the
  access token of Expo is off. With APNs, that also needed the key. So the app trusts
  nothing of a notice but its two ids, which it checks against its own machines. A
  notice starts no action and answers no question.
- A user who builds the app needs an Expo project with a push key for each bundle id.
- An Expo token does not change when iOS installs the app again. A phone that was not
  revoked gets notices again after a new install, before it pairs again. The app drops
  their taps, the 30 days of point 2 end them, and `efr device revoke` ends them at once.
- `efr push test` sends a test notice to each phone with a token and shows the
  receipt of each one, so the user can find a missing push key (`InvalidCredentials`)
  or a dead token (`DeviceNotRegistered`).

## Rejected

- efrd talks to APNs with an imported `.p8` key (the first design). The key is at EAS,
  so a copy on this machine is a new secret to keep and to rotate.
- The access token of Expo ("enhanced push security"). It is an option of the Expo
  account, off until the owner turns it on, and the user's account has it off. With it,
  efrd would keep a secret for push. If the option is on, each send fails with
  `UNAUTHORIZED`, and `efr push test` names it.
