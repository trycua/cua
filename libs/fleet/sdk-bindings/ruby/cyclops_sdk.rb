require_relative "cyclops_sdk/schema"
require_relative "cyclops_sdk/sdk"

module CyclopsSdk
  CyclopsSdkSchema.constants(false).each do |name|
    const_set(name, CyclopsSdkSchema.const_get(name)) unless const_defined?(name, false)
  end
  SCHEMA_CHECK_LOWER_METHODS = %i[
    check_lower_TypeClaimSpec
    check_lower_TypeOSGymSandboxClaimStatus
    check_lower_TypeOSGymWorkspacePoolStatus
    check_lower_TypePoolSpec
  ].freeze
  SCHEMA_READ_METHODS = %i[
    read_TypeClaimSpec
    read_TypeOSGymSandboxClaimStatus
    read_TypeOSGymWorkspacePoolStatus
    read_TypePoolSpec
  ].freeze
  SCHEMA_WRITE_METHODS = %i[
    write_TypeClaimSpec
    write_TypeOSGymSandboxClaimStatus
    write_TypeOSGymWorkspacePoolStatus
    write_TypePoolSpec
  ].freeze

  schema_rust_buffer = CyclopsSdkSchema::RustBuffer
  schema_stream = CyclopsSdkSchema.const_get(:RustBufferStream, false)

  SCHEMA_CHECK_LOWER_METHODS.each do |method_name|
    RustBuffer.define_singleton_method(method_name) do |value|
      schema_rust_buffer.public_send(method_name, value)
    end
  end

  SCHEMA_READ_METHODS.each do |method_name|
    RustBufferStream.define_method(method_name) do
      schema_buffer = schema_rust_buffer.new(@rbuf.pointer)
      external_stream = schema_stream.new(schema_buffer)
      external_stream.instance_variable_set(:@offset, @offset)
      value = external_stream.public_send(method_name)
      @offset = external_stream.instance_variable_get(:@offset)
      value
    end
  end

  SCHEMA_WRITE_METHODS.each do |method_name|
    RustBufferBuilder.define_method(method_name) do |value|
      buffer = schema_rust_buffer.public_send(method_name.to_s.sub("write_", "alloc_from_"), value)
      begin
        write(buffer.data.read_bytes(buffer.len))
      ensure
        buffer.free
      end
    end
  end

  private_constant :SCHEMA_CHECK_LOWER_METHODS, :SCHEMA_READ_METHODS, :SCHEMA_WRITE_METHODS
end
