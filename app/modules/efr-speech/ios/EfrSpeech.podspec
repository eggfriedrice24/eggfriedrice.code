# The local Expo module of voice input. Autolinking finds it in app/modules.
Pod::Spec.new do |s|
  s.name           = 'EfrSpeech'
  s.version        = '0.0.0'
  s.summary        = 'Voice input of the efr app with SpeechAnalyzer'
  s.description    = 'Turns speech into text on the iPhone. The audio does not leave the iPhone.'
  s.license        = 'MIT'
  s.author         = 'eggfriedrice'
  s.homepage       = 'https://docs.expo.dev/modules/'
  # SpeechAnalyzer needs iOS 26. The module builds for older versions too and then says
  # that it is not available.
  s.platforms      = {
    :ios => '16.4'
  }
  s.swift_version  = '5.9'
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = "**/*.{h,m,swift}"
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
