# efr-speech

A local Expo module for iOS. It turns speech into text on the iPhone with SpeechAnalyzer
and SpeechTranscriber of iOS 26. The app puts the text into the draft at the cursor. The
module sends nothing, writes no audio file, and no audio leaves the phone.

## What the module does

- `isAvailable()` is `false` before iOS 26, and when SpeechTranscriber does not run on the
  iPhone or does not support its language (`SpeechTranscriber.supportedLocale`). The app
  then shows no microphone, and the dictation key of the iOS keyboard stays.
- `start()` asks for the microphone. The first time for a language, it gets the speech
  model through AssetInventory and sends the event `phase` with `model`. Then it records
  and sends `phase` with `recording`. When iOS denies the microphone, `start()` fails
  with the code `ERR_MICROPHONE_DENIED` (`MICROPHONE_DENIED` of
  `src/EfrSpeech.types.ts`), and the app shows the line that opens the iOS settings.
- AVAudioEngine gives the buffers of the microphone. The module converts them to the
  format of the analyzer and gives them to it live.
- Each result sends the event `transcript` with the whole text so far: the final parts,
  then the newest part, which a later result can still change.
- `stop()` ends the input, and the analyzer finalizes the last part. The app calls it at
  the stop button and when the user leaves the screen. Two causes stop the recording
  with the reason `interrupted`: a phone call or Siri (an interruption of the audio
  session), and a new audio input such as AirPods or CarPlay (a change of the
  configuration of the audio engine, which stops the engine). Then the last `transcript`
  comes, and `end` with the reason: `stopped`, `interrupted` or `failed`.
- The app limits a recording to 5 minutes. The module has no limit of its own.

The module does not ask for speech recognition. SpeechAnalyzer runs on the iPhone. The
Info.plist still has `NSSpeechRecognitionUsageDescription`, because iOS ends an app that
asks for a permission with no text for it.

## Take the module out of a build

SpeechAnalyzer is new in iOS 26, and Linux cannot compile Swift. If the module does not
compile on EAS, add this to the `expo` object of `app/package.json` and build again:

```json
"autolinking": { "exclude": ["efr-speech"] }
```

`expo-modules-autolinking resolve -p apple` then lists no `efr-speech`. The app shows no
microphone, and the other parts of the app do not change.

## Checks on an EAS build

The Linux binary of expo-modules-macros checks the macros, but not the types:

```sh
node_modules/.pnpm/node_modules/@expo/modules-macros-linux-x64/ExpoModulesMacros \
  expand --diagnostics-only modules/efr-speech/ios/
```

Do these checks on the first EAS development build, on iOS 26 or later:

1. The build log shows the pod `EfrSpeech` and no Swift error in `EfrSpeechModule.swift`.
2. In the app, `requireOptionalNativeModule("EfrSpeech")` is not `null`, and
   `isAvailable()` is `true`.
3. The first `start()` asks for the microphone once. Write down if iOS also asks for
   speech recognition.
4. Speak for 10 s in flight mode. The text comes, and nothing leaves the phone.
5. A phone call during a recording ends it with `interrupted`, and the text so far stays.
