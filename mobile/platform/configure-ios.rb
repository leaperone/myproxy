#!/usr/bin/env ruby
# Reproducibly adds the Packet Tunnel target after Expo prebuild.
require 'xcodeproj'
require 'pathname'
require 'json'

project_path = ARGV[0] || Dir['mobile/**/*.xcodeproj', '*.xcodeproj'].first
abort 'no generated .xcodeproj found; run Expo prebuild first' unless project_path
project = Xcodeproj::Project.open(project_path)
project_dir = Pathname.new(File.expand_path(project_path)).dirname
properties_path = project_dir.join('Podfile.properties.json')
properties = properties_path.exist? ? JSON.parse(properties_path.read) : {}
deployment_target = [Gem::Version.new(properties.fetch('ios.deploymentTarget', '16.4')), Gem::Version.new('16.4')].max.to_s
properties['ios.deploymentTarget'] = deployment_target
properties_path.write(JSON.pretty_generate(properties))
repo_dir = Pathname.new(File.expand_path('.'))
rel = ->(path) { Pathname.new(path).relative_path_from(project_dir).to_s }
app = project.targets.find { |t| t.product_type == 'com.apple.product-type.application' } or abort 'application target not found'
extension = project.targets.find { |t| t.name == 'MyProxyPacketTunnel' }
unless extension
  extension = project.new_target(:app_extension, 'MyProxyPacketTunnel', :ios, deployment_target)
end
extension.build_configurations.each do |config|
  config.build_settings['PRODUCT_BUNDLE_IDENTIFIER'] = 'one.leaper.myproxy.xray.PacketTunnel'
  config.build_settings['INFOPLIST_FILE'] = rel.call(File.join(repo_dir, 'mobile/ios-extension/Info.plist'))
  config.build_settings['CODE_SIGN_ENTITLEMENTS'] = rel.call(File.join(repo_dir, 'mobile/ios-extension/MyProxyPacketTunnel.entitlements'))
  config.build_settings['SWIFT_VERSION'] = '5.0'
  config.build_settings['MARKETING_VERSION'] = '0.0.10'
  config.build_settings['CURRENT_PROJECT_VERSION'] = ENV.fetch('GITHUB_RUN_NUMBER', '1')
  config.build_settings['IPHONEOS_DEPLOYMENT_TARGET'] = deployment_target
  config.build_settings['APPLICATION_EXTENSION_API_ONLY'] = 'YES'
  config.build_settings['OTHER_LDFLAGS'] = ['$(inherited)', '-lresolv', '-framework', 'Security', '-framework', 'CoreFoundation']
end

app.build_configurations.each do |config|
  config.build_settings['CODE_SIGN_ENTITLEMENTS'] = rel.call(File.join(repo_dir, 'mobile/app/modules/myproxy/ios/MyProxy.entitlements'))
  config.build_settings['IPHONEOS_DEPLOYMENT_TARGET'] = deployment_target
end

group = project.main_group.groups.find { |g| g.name == 'MyProxyPacketTunnel' } || project.main_group.new_group('MyProxyPacketTunnel')
source_paths = Dir[File.join(repo_dir, 'mobile/ios-extension/Sources/*.swift')]
source_paths.each do |absolute|
  path = rel.call(absolute)
  ref = project.files.find { |f| f.path == path } || group.new_file(path)
  extension.sources_build_phase.add_file_reference(ref) unless extension.sources_build_phase.files.any? { |b| b.file_ref == ref }
end

framework_paths = %w[MyProxyCore.xcframework MyProxyNetwork.xcframework].map do |name|
  File.join(repo_dir, 'mobile/app/modules/myproxy/ios/Frameworks', name)
end
framework_paths.each do |absolute|
  path = rel.call(absolute)
  ref = project.files.find { |f| f.path == path } || project.main_group.new_file(path)
  extension.frameworks_build_phase.add_file_reference(ref) unless extension.frameworks_build_phase.files.any? { |b| b.file_ref == ref }
end

app.add_dependency(extension) unless app.dependencies.any? { |d| d.target == extension }
phase = app.copy_files_build_phases.find { |p| p.name == 'Embed Packet Tunnel' } || app.new_copy_files_build_phase('Embed Packet Tunnel')
phase.dst_subfolder_spec = '13'
phase.add_file_reference(extension.product_reference) unless phase.files.any? { |b| b.file_ref == extension.product_reference }
project.save
