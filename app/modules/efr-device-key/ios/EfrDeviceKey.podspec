# The local Expo module of the device key. Autolinking finds it in app/modules.
Pod::Spec.new do |s|
  s.name           = 'EfrDeviceKey'
  s.version        = '0.0.0'
  s.summary        = 'The P-256 device key of the efr app, in the Secure Enclave'
  s.description    = 'Makes, keeps and uses the P-256 key that proves the phone to efrd.'
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
