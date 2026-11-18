# The local Expo module of the attached files. Autolinking finds it in app/modules.
Pod::Spec.new do |s|
  s.name           = 'EfrFiles'
  s.version        = '0.0.0'
  s.summary        = 'The bytes of the files that the efr app attaches to a prompt'
  s.description    = 'Makes a photo small with no metadata, reads a file in chunks and removes the copies.'
  s.license        = 'MIT'
  s.author         = 'eggfriedrice'
  s.homepage       = 'https://docs.expo.dev/modules/'
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
