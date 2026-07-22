package cyclops_sdk_schema

// #include <cyclops_sdk_schema.h>
import "C"

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"io"
	"math"
	"runtime"
	"sync/atomic"
	"unsafe"
)

// This is needed, because as of go 1.24
// type RustBuffer C.RustBuffer cannot have methods,
// RustBuffer is treated as non-local type
type GoRustBuffer struct {
	inner C.RustBuffer
}

type RustBufferI interface {
	AsReader() *bytes.Reader
	Free()
	ToGoBytes() []byte
	Data() unsafe.Pointer
	Len() uint64
	Capacity() uint64
}

// C.RustBuffer fields exposed as an interface so they can be accessed in different Go packages.
// See https://github.com/golang/go/issues/13467
type ExternalCRustBuffer interface {
	Data() unsafe.Pointer
	Len() uint64
	Capacity() uint64
}

func RustBufferFromC(b C.RustBuffer) ExternalCRustBuffer {
	return GoRustBuffer{
		inner: b,
	}
}

func CFromRustBuffer(b ExternalCRustBuffer) C.RustBuffer {
	return C.RustBuffer{
		capacity: C.uint64_t(b.Capacity()),
		len:      C.uint64_t(b.Len()),
		data:     (*C.uchar)(b.Data()),
	}
}

func RustBufferFromExternal(b ExternalCRustBuffer) GoRustBuffer {
	return GoRustBuffer{
		inner: C.RustBuffer{
			capacity: C.uint64_t(b.Capacity()),
			len:      C.uint64_t(b.Len()),
			data:     (*C.uchar)(b.Data()),
		},
	}
}

func (cb GoRustBuffer) Capacity() uint64 {
	return uint64(cb.inner.capacity)
}

func (cb GoRustBuffer) Len() uint64 {
	return uint64(cb.inner.len)
}

func (cb GoRustBuffer) Data() unsafe.Pointer {
	return unsafe.Pointer(cb.inner.data)
}

func (cb GoRustBuffer) AsReader() *bytes.Reader {
	b := unsafe.Slice((*byte)(cb.inner.data), C.uint64_t(cb.inner.len))
	return bytes.NewReader(b)
}

func (cb GoRustBuffer) Free() {
	rustCall(func(status *C.RustCallStatus) bool {
		C.ffi_cyclops_sdk_schema_rustbuffer_free(cb.inner, status)
		return false
	})
}

func (cb GoRustBuffer) ToGoBytes() []byte {
	return C.GoBytes(unsafe.Pointer(cb.inner.data), C.int(cb.inner.len))
}

func stringToRustBuffer(str string) C.RustBuffer {
	return bytesToRustBuffer([]byte(str))
}

func bytesToRustBuffer(b []byte) C.RustBuffer {
	if len(b) == 0 {
		return C.RustBuffer{}
	}
	// We can pass the pointer along here, as it is pinned
	// for the duration of this call
	foreign := C.ForeignBytes{
		len:  C.int(len(b)),
		data: (*C.uchar)(unsafe.Pointer(&b[0])),
	}

	return rustCall(func(status *C.RustCallStatus) C.RustBuffer {
		return C.ffi_cyclops_sdk_schema_rustbuffer_from_bytes(foreign, status)
	})
}

type BufLifter[GoType any] interface {
	Lift(value RustBufferI) GoType
}

type BufLowerer[GoType any] interface {
	Lower(value GoType) C.RustBuffer
}

type BufReader[GoType any] interface {
	Read(reader io.Reader) GoType
}

type BufWriter[GoType any] interface {
	Write(writer io.Writer, value GoType)
}

func LowerIntoRustBuffer[GoType any](bufWriter BufWriter[GoType], value GoType) C.RustBuffer {
	// This might be not the most efficient way but it does not require knowing allocation size
	// beforehand
	var buffer bytes.Buffer
	bufWriter.Write(&buffer, value)

	bytes, err := io.ReadAll(&buffer)
	if err != nil {
		panic(fmt.Errorf("reading written data: %w", err))
	}
	return bytesToRustBuffer(bytes)
}

func LiftFromRustBuffer[GoType any](bufReader BufReader[GoType], rbuf RustBufferI) GoType {
	defer rbuf.Free()
	reader := rbuf.AsReader()
	item := bufReader.Read(reader)
	if reader.Len() > 0 {
		// TODO: Remove this
		leftover, _ := io.ReadAll(reader)
		panic(fmt.Errorf("Junk remaining in buffer after lifting: %s", string(leftover)))
	}
	return item
}

func rustCallWithError[E any, U any](converter BufReader[E], callback func(*C.RustCallStatus) U) (U, E) {
	var status C.RustCallStatus
	returnValue := callback(&status)
	err := checkCallStatus(converter, status)
	return returnValue, err
}

func checkCallStatus[E any](converter BufReader[E], status C.RustCallStatus) E {
	switch status.code {
	case 0:
		var zero E
		return zero
	case 1:
		return LiftFromRustBuffer(converter, GoRustBuffer{inner: status.errorBuf})
	case 2:
		// when the rust code sees a panic, it tries to construct a rustBuffer
		// with the message.  but if that code panics, then it just sends back
		// an empty buffer.
		if status.errorBuf.len > 0 {
			panic(fmt.Errorf("%s", FfiConverterStringINSTANCE.Lift(GoRustBuffer{inner: status.errorBuf})))
		} else {
			panic(fmt.Errorf("Rust panicked while handling Rust panic"))
		}
	default:
		panic(fmt.Errorf("unknown status code: %d", status.code))
	}
}

func checkCallStatusUnknown(status C.RustCallStatus) error {
	switch status.code {
	case 0:
		return nil
	case 1:
		panic(fmt.Errorf("function not returning an error returned an error"))
	case 2:
		// when the rust code sees a panic, it tries to construct a C.RustBuffer
		// with the message.  but if that code panics, then it just sends back
		// an empty buffer.
		if status.errorBuf.len > 0 {
			panic(fmt.Errorf("%s", FfiConverterStringINSTANCE.Lift(GoRustBuffer{
				inner: status.errorBuf,
			})))
		} else {
			panic(fmt.Errorf("Rust panicked while handling Rust panic"))
		}
	default:
		return fmt.Errorf("unknown status code: %d", status.code)
	}
}

func rustCall[U any](callback func(*C.RustCallStatus) U) U {
	returnValue, err := rustCallWithError[error](nil, callback)
	if err != nil {
		panic(err)
	}
	return returnValue
}

type NativeError interface {
	AsError() error
}

func writeInt8(writer io.Writer, value int8) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint8(writer io.Writer, value uint8) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeInt16(writer io.Writer, value int16) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint16(writer io.Writer, value uint16) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeInt32(writer io.Writer, value int32) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint32(writer io.Writer, value uint32) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeInt64(writer io.Writer, value int64) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeUint64(writer io.Writer, value uint64) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeFloat32(writer io.Writer, value float32) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func writeFloat64(writer io.Writer, value float64) {
	if err := binary.Write(writer, binary.BigEndian, value); err != nil {
		panic(err)
	}
}

func readInt8(reader io.Reader) int8 {
	var result int8
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint8(reader io.Reader) uint8 {
	var result uint8
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readInt16(reader io.Reader) int16 {
	var result int16
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint16(reader io.Reader) uint16 {
	var result uint16
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readInt32(reader io.Reader) int32 {
	var result int32
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint32(reader io.Reader) uint32 {
	var result uint32
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readInt64(reader io.Reader) int64 {
	var result int64
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readUint64(reader io.Reader) uint64 {
	var result uint64
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readFloat32(reader io.Reader) float32 {
	var result float32
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func readFloat64(reader io.Reader) float64 {
	var result float64
	if err := binary.Read(reader, binary.BigEndian, &result); err != nil {
		panic(err)
	}
	return result
}

func init() {

	uniffiCheckChecksums()
}

func uniffiCheckChecksums() {
	// Get the bindings contract version from our ComponentInterface
	bindingsContractVersion := 30
	// Get the scaffolding contract version by calling the into the dylib
	scaffoldingContractVersion := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint32_t {
		return C.ffi_cyclops_sdk_schema_uniffi_contract_version()
	})
	if bindingsContractVersion != int(scaffoldingContractVersion) {
		// If this happens try cleaning and rebuilding your project
		panic("cyclops_sdk_schema: UniFFI contract version mismatch")
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json()
		})
		if checksum != 8252 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk_schema: uniffi_cyclops_sdk_schema_checksum_method_preservedjson_to_json: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json()
		})
		if checksum != 24064 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk_schema: uniffi_cyclops_sdk_schema_checksum_constructor_preservedjson_from_json: UniFFI API checksum mismatch")
		}
	}
}

type FfiConverterUint16 struct{}

var FfiConverterUint16INSTANCE = FfiConverterUint16{}

func (FfiConverterUint16) Lower(value uint16) C.uint16_t {
	return C.uint16_t(value)
}

func (FfiConverterUint16) Write(writer io.Writer, value uint16) {
	writeUint16(writer, value)
}

func (FfiConverterUint16) Lift(value C.uint16_t) uint16 {
	return uint16(value)
}

func (FfiConverterUint16) Read(reader io.Reader) uint16 {
	return readUint16(reader)
}

type FfiDestroyerUint16 struct{}

func (FfiDestroyerUint16) Destroy(_ uint16) {}

type FfiConverterUint32 struct{}

var FfiConverterUint32INSTANCE = FfiConverterUint32{}

func (FfiConverterUint32) Lower(value uint32) C.uint32_t {
	return C.uint32_t(value)
}

func (FfiConverterUint32) Write(writer io.Writer, value uint32) {
	writeUint32(writer, value)
}

func (FfiConverterUint32) Lift(value C.uint32_t) uint32 {
	return uint32(value)
}

func (FfiConverterUint32) Read(reader io.Reader) uint32 {
	return readUint32(reader)
}

type FfiDestroyerUint32 struct{}

func (FfiDestroyerUint32) Destroy(_ uint32) {}

type FfiConverterUint64 struct{}

var FfiConverterUint64INSTANCE = FfiConverterUint64{}

func (FfiConverterUint64) Lower(value uint64) C.uint64_t {
	return C.uint64_t(value)
}

func (FfiConverterUint64) Write(writer io.Writer, value uint64) {
	writeUint64(writer, value)
}

func (FfiConverterUint64) Lift(value C.uint64_t) uint64 {
	return uint64(value)
}

func (FfiConverterUint64) Read(reader io.Reader) uint64 {
	return readUint64(reader)
}

type FfiDestroyerUint64 struct{}

func (FfiDestroyerUint64) Destroy(_ uint64) {}

type FfiConverterBool struct{}

var FfiConverterBoolINSTANCE = FfiConverterBool{}

func (FfiConverterBool) Lower(value bool) C.int8_t {
	if value {
		return C.int8_t(1)
	}
	return C.int8_t(0)
}

func (FfiConverterBool) Write(writer io.Writer, value bool) {
	if value {
		writeInt8(writer, 1)
	} else {
		writeInt8(writer, 0)
	}
}

func (FfiConverterBool) Lift(value C.int8_t) bool {
	return value != 0
}

func (FfiConverterBool) Read(reader io.Reader) bool {
	return readInt8(reader) != 0
}

type FfiDestroyerBool struct{}

func (FfiDestroyerBool) Destroy(_ bool) {}

type FfiConverterString struct{}

var FfiConverterStringINSTANCE = FfiConverterString{}

func (FfiConverterString) Lift(rb RustBufferI) string {
	defer rb.Free()
	reader := rb.AsReader()
	b, err := io.ReadAll(reader)
	if err != nil {
		panic(fmt.Errorf("reading reader: %w", err))
	}
	return string(b)
}

func (FfiConverterString) Read(reader io.Reader) string {
	length := readInt32(reader)
	buffer := make([]byte, length)
	read_length, err := reader.Read(buffer)
	if err != nil && err != io.EOF {
		panic(err)
	}
	if read_length != int(length) {
		panic(fmt.Errorf("bad read length when reading string, expected %d, read %d", length, read_length))
	}
	return string(buffer)
}

func (FfiConverterString) Lower(value string) C.RustBuffer {
	return stringToRustBuffer(value)
}

func (c FfiConverterString) LowerExternal(value string) ExternalCRustBuffer {
	return RustBufferFromC(stringToRustBuffer(value))
}

func (FfiConverterString) Write(writer io.Writer, value string) {
	if len(value) > math.MaxInt32 {
		panic("String is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	write_length, err := io.WriteString(writer, value)
	if err != nil {
		panic(err)
	}
	if write_length != len(value) {
		panic(fmt.Errorf("bad write length when writing string, expected %d, written %d", len(value), write_length))
	}
}

type FfiDestroyerString struct{}

func (FfiDestroyerString) Destroy(_ string) {}

// Below is an implementation of synchronization requirements outlined in the link.
// https://github.com/mozilla/uniffi-rs/blob/0dc031132d9493ca812c3af6e7dd60ad2ea95bf0/uniffi_bindgen/src/bindings/kotlin/templates/ObjectRuntime.kt#L31

type FfiObject struct {
	handle        C.uint64_t
	callCounter   atomic.Int64
	cloneFunction func(C.uint64_t, *C.RustCallStatus) C.uint64_t
	freeFunction  func(C.uint64_t, *C.RustCallStatus)
	destroyed     atomic.Bool
}

func newFfiObject(
	handle C.uint64_t,
	cloneFunction func(C.uint64_t, *C.RustCallStatus) C.uint64_t,
	freeFunction func(C.uint64_t, *C.RustCallStatus),
) FfiObject {
	return FfiObject{
		handle:        handle,
		cloneFunction: cloneFunction,
		freeFunction:  freeFunction,
	}
}

func (ffiObject *FfiObject) incrementPointer(debugName string) C.uint64_t {
	for {
		counter := ffiObject.callCounter.Load()
		if counter <= -1 {
			panic(fmt.Errorf("%v object has already been destroyed", debugName))
		}
		if counter == math.MaxInt64 {
			panic(fmt.Errorf("%v object call counter would overflow", debugName))
		}
		if ffiObject.callCounter.CompareAndSwap(counter, counter+1) {
			break
		}
	}

	return rustCall(func(status *C.RustCallStatus) C.uint64_t {
		return ffiObject.cloneFunction(ffiObject.handle, status)
	})
}

func (ffiObject *FfiObject) decrementPointer() {
	if ffiObject.callCounter.Add(-1) == -1 {
		ffiObject.freeRustArcPtr()
	}
}

func (ffiObject *FfiObject) destroy() {
	if ffiObject.destroyed.CompareAndSwap(false, true) {
		if ffiObject.callCounter.Add(-1) == -1 {
			ffiObject.freeRustArcPtr()
		}
	}
}

func (ffiObject *FfiObject) freeRustArcPtr() {
	if ffiObject.handle == 0 {
		return
	}
	rustCall(func(status *C.RustCallStatus) int32 {
		ffiObject.freeFunction(ffiObject.handle, status)
		return 0
	})
}

type PreservedJsonInterface interface {
	ToJson() string
}
type PreservedJson struct {
	ffiObject FfiObject
}

func PreservedJsonFromJson(value string) (*PreservedJson, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*JsonValueError](FfiConverterJsonValueError{}, func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_cyclops_sdk_schema_fn_constructor_preservedjson_from_json(FfiConverterStringINSTANCE.Lower(value), _uniffiStatus)
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *PreservedJson
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterPreservedJsonINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *PreservedJson) ToJson() string {
	_pointer := _self.ffiObject.incrementPointer("*PreservedJson")
	defer _self.ffiObject.decrementPointer()
	return FfiConverterStringINSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) RustBufferI {
		return GoRustBuffer{
			inner: C.uniffi_cyclops_sdk_schema_fn_method_preservedjson_to_json(
				_pointer, _uniffiStatus),
		}
	}))
}
func (object *PreservedJson) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterPreservedJson struct{}

var FfiConverterPreservedJsonINSTANCE = FfiConverterPreservedJson{}

func (c FfiConverterPreservedJson) Lift(handle C.uint64_t) *PreservedJson {
	result := &PreservedJson{
		newFfiObject(
			handle,
			func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
				return C.uniffi_cyclops_sdk_schema_fn_clone_preservedjson(handle, status)
			},
			func(handle C.uint64_t, status *C.RustCallStatus) {
				C.uniffi_cyclops_sdk_schema_fn_free_preservedjson(handle, status)
			},
		),
	}
	runtime.SetFinalizer(result, (*PreservedJson).Destroy)
	return result
}

func (c FfiConverterPreservedJson) Read(reader io.Reader) *PreservedJson {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterPreservedJson) Lower(value *PreservedJson) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	handle := value.ffiObject.incrementPointer("*PreservedJson")
	defer value.ffiObject.decrementPointer()
	return handle
}

func (c FfiConverterPreservedJson) Write(writer io.Writer, value *PreservedJson) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalPreservedJson(handle uint64) *PreservedJson {
	return FfiConverterPreservedJsonINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalPreservedJson(value *PreservedJson) uint64 {
	return uint64(FfiConverterPreservedJsonINSTANCE.Lower(value))
}

type FfiDestroyerPreservedJson struct{}

func (_ FfiDestroyerPreservedJson) Destroy(value *PreservedJson) {
	value.Destroy()
}

type ClaimLifecycle struct {
	ShutdownTime   *string
	ShutdownPolicy *string
	AutoRenew      *bool
}

func (r *ClaimLifecycle) Destroy() {
	FfiDestroyerOptionalString{}.Destroy(r.ShutdownTime)
	FfiDestroyerOptionalString{}.Destroy(r.ShutdownPolicy)
	FfiDestroyerOptionalBool{}.Destroy(r.AutoRenew)
}

type FfiConverterClaimLifecycle struct{}

var FfiConverterClaimLifecycleINSTANCE = FfiConverterClaimLifecycle{}

func (c FfiConverterClaimLifecycle) Lift(rb RustBufferI) ClaimLifecycle {
	return LiftFromRustBuffer[ClaimLifecycle](c, rb)
}

func (c FfiConverterClaimLifecycle) Read(reader io.Reader) ClaimLifecycle {
	return ClaimLifecycle{
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalBoolINSTANCE.Read(reader),
	}
}

func (c FfiConverterClaimLifecycle) Lower(value ClaimLifecycle) C.RustBuffer {
	return LowerIntoRustBuffer[ClaimLifecycle](c, value)
}

func (c FfiConverterClaimLifecycle) LowerExternal(value ClaimLifecycle) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ClaimLifecycle](c, value))
}

func (c FfiConverterClaimLifecycle) Write(writer io.Writer, value ClaimLifecycle) {
	FfiConverterOptionalStringINSTANCE.Write(writer, value.ShutdownTime)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.ShutdownPolicy)
	FfiConverterOptionalBoolINSTANCE.Write(writer, value.AutoRenew)
}

type FfiDestroyerClaimLifecycle struct{}

func (_ FfiDestroyerClaimLifecycle) Destroy(value ClaimLifecycle) {
	value.Destroy()
}

type ClaimSpec struct {
	SandboxTemplateRef SandboxTemplateRef
	Warmpool           *string
	BindDeadline       *uint32
	Lifecycle          *ClaimLifecycle
}

func (r *ClaimSpec) Destroy() {
	FfiDestroyerSandboxTemplateRef{}.Destroy(r.SandboxTemplateRef)
	FfiDestroyerOptionalString{}.Destroy(r.Warmpool)
	FfiDestroyerOptionalUint32{}.Destroy(r.BindDeadline)
	FfiDestroyerOptionalClaimLifecycle{}.Destroy(r.Lifecycle)
}

type FfiConverterClaimSpec struct{}

var FfiConverterClaimSpecINSTANCE = FfiConverterClaimSpec{}

func (c FfiConverterClaimSpec) Lift(rb RustBufferI) ClaimSpec {
	return LiftFromRustBuffer[ClaimSpec](c, rb)
}

func (c FfiConverterClaimSpec) Read(reader io.Reader) ClaimSpec {
	return ClaimSpec{
		FfiConverterSandboxTemplateRefINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalClaimLifecycleINSTANCE.Read(reader),
	}
}

func (c FfiConverterClaimSpec) Lower(value ClaimSpec) C.RustBuffer {
	return LowerIntoRustBuffer[ClaimSpec](c, value)
}

func (c FfiConverterClaimSpec) LowerExternal(value ClaimSpec) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ClaimSpec](c, value))
}

func (c FfiConverterClaimSpec) Write(writer io.Writer, value ClaimSpec) {
	FfiConverterSandboxTemplateRefINSTANCE.Write(writer, value.SandboxTemplateRef)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Warmpool)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.BindDeadline)
	FfiConverterOptionalClaimLifecycleINSTANCE.Write(writer, value.Lifecycle)
}

type FfiDestroyerClaimSpec struct{}

func (_ FfiDestroyerClaimSpec) Destroy(value ClaimSpec) {
	value.Destroy()
}

type OsGymSandboxClaimCondition struct {
	Type               *string
	Status             *string
	Reason             *string
	Message            *string
	LastTransitionTime *string
}

func (r *OsGymSandboxClaimCondition) Destroy() {
	FfiDestroyerOptionalString{}.Destroy(r.Type)
	FfiDestroyerOptionalString{}.Destroy(r.Status)
	FfiDestroyerOptionalString{}.Destroy(r.Reason)
	FfiDestroyerOptionalString{}.Destroy(r.Message)
	FfiDestroyerOptionalString{}.Destroy(r.LastTransitionTime)
}

type FfiConverterOsGymSandboxClaimCondition struct{}

var FfiConverterOsGymSandboxClaimConditionINSTANCE = FfiConverterOsGymSandboxClaimCondition{}

func (c FfiConverterOsGymSandboxClaimCondition) Lift(rb RustBufferI) OsGymSandboxClaimCondition {
	return LiftFromRustBuffer[OsGymSandboxClaimCondition](c, rb)
}

func (c FfiConverterOsGymSandboxClaimCondition) Read(reader io.Reader) OsGymSandboxClaimCondition {
	return OsGymSandboxClaimCondition{
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxClaimCondition) Lower(value OsGymSandboxClaimCondition) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxClaimCondition](c, value)
}

func (c FfiConverterOsGymSandboxClaimCondition) LowerExternal(value OsGymSandboxClaimCondition) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxClaimCondition](c, value))
}

func (c FfiConverterOsGymSandboxClaimCondition) Write(writer io.Writer, value OsGymSandboxClaimCondition) {
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Type)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Status)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Reason)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Message)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.LastTransitionTime)
}

type FfiDestroyerOsGymSandboxClaimCondition struct{}

func (_ FfiDestroyerOsGymSandboxClaimCondition) Destroy(value OsGymSandboxClaimCondition) {
	value.Destroy()
}

type OsGymSandboxClaimSandbox struct {
	Name    *string
	Service *string
}

func (r *OsGymSandboxClaimSandbox) Destroy() {
	FfiDestroyerOptionalString{}.Destroy(r.Name)
	FfiDestroyerOptionalString{}.Destroy(r.Service)
}

type FfiConverterOsGymSandboxClaimSandbox struct{}

var FfiConverterOsGymSandboxClaimSandboxINSTANCE = FfiConverterOsGymSandboxClaimSandbox{}

func (c FfiConverterOsGymSandboxClaimSandbox) Lift(rb RustBufferI) OsGymSandboxClaimSandbox {
	return LiftFromRustBuffer[OsGymSandboxClaimSandbox](c, rb)
}

func (c FfiConverterOsGymSandboxClaimSandbox) Read(reader io.Reader) OsGymSandboxClaimSandbox {
	return OsGymSandboxClaimSandbox{
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxClaimSandbox) Lower(value OsGymSandboxClaimSandbox) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxClaimSandbox](c, value)
}

func (c FfiConverterOsGymSandboxClaimSandbox) LowerExternal(value OsGymSandboxClaimSandbox) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxClaimSandbox](c, value))
}

func (c FfiConverterOsGymSandboxClaimSandbox) Write(writer io.Writer, value OsGymSandboxClaimSandbox) {
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Name)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Service)
}

type FfiDestroyerOsGymSandboxClaimSandbox struct{}

func (_ FfiDestroyerOsGymSandboxClaimSandbox) Destroy(value OsGymSandboxClaimSandbox) {
	value.Destroy()
}

type OsGymSandboxClaimStatus struct {
	Phase      *string
	Conditions *[]OsGymSandboxClaimCondition
	Sandbox    *OsGymSandboxClaimSandbox
}

func (r *OsGymSandboxClaimStatus) Destroy() {
	FfiDestroyerOptionalString{}.Destroy(r.Phase)
	FfiDestroyerOptionalSequenceOsGymSandboxClaimCondition{}.Destroy(r.Conditions)
	FfiDestroyerOptionalOsGymSandboxClaimSandbox{}.Destroy(r.Sandbox)
}

type FfiConverterOsGymSandboxClaimStatus struct{}

var FfiConverterOsGymSandboxClaimStatusINSTANCE = FfiConverterOsGymSandboxClaimStatus{}

func (c FfiConverterOsGymSandboxClaimStatus) Lift(rb RustBufferI) OsGymSandboxClaimStatus {
	return LiftFromRustBuffer[OsGymSandboxClaimStatus](c, rb)
}

func (c FfiConverterOsGymSandboxClaimStatus) Read(reader io.Reader) OsGymSandboxClaimStatus {
	return OsGymSandboxClaimStatus{
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalSequenceOsGymSandboxClaimConditionINSTANCE.Read(reader),
		FfiConverterOptionalOsGymSandboxClaimSandboxINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxClaimStatus) Lower(value OsGymSandboxClaimStatus) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxClaimStatus](c, value)
}

func (c FfiConverterOsGymSandboxClaimStatus) LowerExternal(value OsGymSandboxClaimStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxClaimStatus](c, value))
}

func (c FfiConverterOsGymSandboxClaimStatus) Write(writer io.Writer, value OsGymSandboxClaimStatus) {
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Phase)
	FfiConverterOptionalSequenceOsGymSandboxClaimConditionINSTANCE.Write(writer, value.Conditions)
	FfiConverterOptionalOsGymSandboxClaimSandboxINSTANCE.Write(writer, value.Sandbox)
}

type FfiDestroyerOsGymSandboxClaimStatus struct{}

func (_ FfiDestroyerOsGymSandboxClaimStatus) Destroy(value OsGymSandboxClaimStatus) {
	value.Destroy()
}

type OsGymSandboxSpec struct {
	VmTemplate VmTemplate
}

func (r *OsGymSandboxSpec) Destroy() {
	FfiDestroyerVmTemplate{}.Destroy(r.VmTemplate)
}

type FfiConverterOsGymSandboxSpec struct{}

var FfiConverterOsGymSandboxSpecINSTANCE = FfiConverterOsGymSandboxSpec{}

func (c FfiConverterOsGymSandboxSpec) Lift(rb RustBufferI) OsGymSandboxSpec {
	return LiftFromRustBuffer[OsGymSandboxSpec](c, rb)
}

func (c FfiConverterOsGymSandboxSpec) Read(reader io.Reader) OsGymSandboxSpec {
	return OsGymSandboxSpec{
		FfiConverterVmTemplateINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxSpec) Lower(value OsGymSandboxSpec) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxSpec](c, value)
}

func (c FfiConverterOsGymSandboxSpec) LowerExternal(value OsGymSandboxSpec) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxSpec](c, value))
}

func (c FfiConverterOsGymSandboxSpec) Write(writer io.Writer, value OsGymSandboxSpec) {
	FfiConverterVmTemplateINSTANCE.Write(writer, value.VmTemplate)
}

type FfiDestroyerOsGymSandboxSpec struct{}

func (_ FfiDestroyerOsGymSandboxSpec) Destroy(value OsGymSandboxSpec) {
	value.Destroy()
}

type OsGymSandboxStatus struct {
	Phase         *string
	Runtime       *string
	Ready         *bool
	VmName        *string
	Service       *string
	Message       *string
	ResetIssuedAt *string
	ResetVmiUid   *string
}

func (r *OsGymSandboxStatus) Destroy() {
	FfiDestroyerOptionalString{}.Destroy(r.Phase)
	FfiDestroyerOptionalString{}.Destroy(r.Runtime)
	FfiDestroyerOptionalBool{}.Destroy(r.Ready)
	FfiDestroyerOptionalString{}.Destroy(r.VmName)
	FfiDestroyerOptionalString{}.Destroy(r.Service)
	FfiDestroyerOptionalString{}.Destroy(r.Message)
	FfiDestroyerOptionalString{}.Destroy(r.ResetIssuedAt)
	FfiDestroyerOptionalString{}.Destroy(r.ResetVmiUid)
}

type FfiConverterOsGymSandboxStatus struct{}

var FfiConverterOsGymSandboxStatusINSTANCE = FfiConverterOsGymSandboxStatus{}

func (c FfiConverterOsGymSandboxStatus) Lift(rb RustBufferI) OsGymSandboxStatus {
	return LiftFromRustBuffer[OsGymSandboxStatus](c, rb)
}

func (c FfiConverterOsGymSandboxStatus) Read(reader io.Reader) OsGymSandboxStatus {
	return OsGymSandboxStatus{
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalBoolINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxStatus) Lower(value OsGymSandboxStatus) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxStatus](c, value)
}

func (c FfiConverterOsGymSandboxStatus) LowerExternal(value OsGymSandboxStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxStatus](c, value))
}

func (c FfiConverterOsGymSandboxStatus) Write(writer io.Writer, value OsGymSandboxStatus) {
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Phase)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Runtime)
	FfiConverterOptionalBoolINSTANCE.Write(writer, value.Ready)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.VmName)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Service)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Message)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.ResetIssuedAt)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.ResetVmiUid)
}

type FfiDestroyerOsGymSandboxStatus struct{}

func (_ FfiDestroyerOsGymSandboxStatus) Destroy(value OsGymSandboxStatus) {
	value.Destroy()
}

type OsGymSandboxTemplateSpec struct {
	VmTemplate VmTemplate
}

func (r *OsGymSandboxTemplateSpec) Destroy() {
	FfiDestroyerVmTemplate{}.Destroy(r.VmTemplate)
}

type FfiConverterOsGymSandboxTemplateSpec struct{}

var FfiConverterOsGymSandboxTemplateSpecINSTANCE = FfiConverterOsGymSandboxTemplateSpec{}

func (c FfiConverterOsGymSandboxTemplateSpec) Lift(rb RustBufferI) OsGymSandboxTemplateSpec {
	return LiftFromRustBuffer[OsGymSandboxTemplateSpec](c, rb)
}

func (c FfiConverterOsGymSandboxTemplateSpec) Read(reader io.Reader) OsGymSandboxTemplateSpec {
	return OsGymSandboxTemplateSpec{
		FfiConverterVmTemplateINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxTemplateSpec) Lower(value OsGymSandboxTemplateSpec) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxTemplateSpec](c, value)
}

func (c FfiConverterOsGymSandboxTemplateSpec) LowerExternal(value OsGymSandboxTemplateSpec) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxTemplateSpec](c, value))
}

func (c FfiConverterOsGymSandboxTemplateSpec) Write(writer io.Writer, value OsGymSandboxTemplateSpec) {
	FfiConverterVmTemplateINSTANCE.Write(writer, value.VmTemplate)
}

type FfiDestroyerOsGymSandboxTemplateSpec struct{}

func (_ FfiDestroyerOsGymSandboxTemplateSpec) Destroy(value OsGymSandboxTemplateSpec) {
	value.Destroy()
}

type OsGymSandboxWarmPoolSpec struct {
	Replicas           uint32
	SandboxTemplateRef SandboxTemplateRef
	Autoscaling        *WarmPoolAutoscaling
}

func (r *OsGymSandboxWarmPoolSpec) Destroy() {
	FfiDestroyerUint32{}.Destroy(r.Replicas)
	FfiDestroyerSandboxTemplateRef{}.Destroy(r.SandboxTemplateRef)
	FfiDestroyerOptionalWarmPoolAutoscaling{}.Destroy(r.Autoscaling)
}

type FfiConverterOsGymSandboxWarmPoolSpec struct{}

var FfiConverterOsGymSandboxWarmPoolSpecINSTANCE = FfiConverterOsGymSandboxWarmPoolSpec{}

func (c FfiConverterOsGymSandboxWarmPoolSpec) Lift(rb RustBufferI) OsGymSandboxWarmPoolSpec {
	return LiftFromRustBuffer[OsGymSandboxWarmPoolSpec](c, rb)
}

func (c FfiConverterOsGymSandboxWarmPoolSpec) Read(reader io.Reader) OsGymSandboxWarmPoolSpec {
	return OsGymSandboxWarmPoolSpec{
		FfiConverterUint32INSTANCE.Read(reader),
		FfiConverterSandboxTemplateRefINSTANCE.Read(reader),
		FfiConverterOptionalWarmPoolAutoscalingINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxWarmPoolSpec) Lower(value OsGymSandboxWarmPoolSpec) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxWarmPoolSpec](c, value)
}

func (c FfiConverterOsGymSandboxWarmPoolSpec) LowerExternal(value OsGymSandboxWarmPoolSpec) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxWarmPoolSpec](c, value))
}

func (c FfiConverterOsGymSandboxWarmPoolSpec) Write(writer io.Writer, value OsGymSandboxWarmPoolSpec) {
	FfiConverterUint32INSTANCE.Write(writer, value.Replicas)
	FfiConverterSandboxTemplateRefINSTANCE.Write(writer, value.SandboxTemplateRef)
	FfiConverterOptionalWarmPoolAutoscalingINSTANCE.Write(writer, value.Autoscaling)
}

type FfiDestroyerOsGymSandboxWarmPoolSpec struct{}

func (_ FfiDestroyerOsGymSandboxWarmPoolSpec) Destroy(value OsGymSandboxWarmPoolSpec) {
	value.Destroy()
}

type OsGymSandboxWarmPoolStatus struct {
	Replicas      *uint32
	ReadyReplicas *uint32
	Selector      *string
}

func (r *OsGymSandboxWarmPoolStatus) Destroy() {
	FfiDestroyerOptionalUint32{}.Destroy(r.Replicas)
	FfiDestroyerOptionalUint32{}.Destroy(r.ReadyReplicas)
	FfiDestroyerOptionalString{}.Destroy(r.Selector)
}

type FfiConverterOsGymSandboxWarmPoolStatus struct{}

var FfiConverterOsGymSandboxWarmPoolStatusINSTANCE = FfiConverterOsGymSandboxWarmPoolStatus{}

func (c FfiConverterOsGymSandboxWarmPoolStatus) Lift(rb RustBufferI) OsGymSandboxWarmPoolStatus {
	return LiftFromRustBuffer[OsGymSandboxWarmPoolStatus](c, rb)
}

func (c FfiConverterOsGymSandboxWarmPoolStatus) Read(reader io.Reader) OsGymSandboxWarmPoolStatus {
	return OsGymSandboxWarmPoolStatus{
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymSandboxWarmPoolStatus) Lower(value OsGymSandboxWarmPoolStatus) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymSandboxWarmPoolStatus](c, value)
}

func (c FfiConverterOsGymSandboxWarmPoolStatus) LowerExternal(value OsGymSandboxWarmPoolStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymSandboxWarmPoolStatus](c, value))
}

func (c FfiConverterOsGymSandboxWarmPoolStatus) Write(writer io.Writer, value OsGymSandboxWarmPoolStatus) {
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.Replicas)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.ReadyReplicas)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Selector)
}

type FfiDestroyerOsGymSandboxWarmPoolStatus struct{}

func (_ FfiDestroyerOsGymSandboxWarmPoolStatus) Destroy(value OsGymSandboxWarmPoolStatus) {
	value.Destroy()
}

type OsGymWorkspacePoolStatus struct {
	Phase          *string
	TotalCount     *uint32
	AvailableCount *uint32
	ClaimedCount   *uint32
}

func (r *OsGymWorkspacePoolStatus) Destroy() {
	FfiDestroyerOptionalString{}.Destroy(r.Phase)
	FfiDestroyerOptionalUint32{}.Destroy(r.TotalCount)
	FfiDestroyerOptionalUint32{}.Destroy(r.AvailableCount)
	FfiDestroyerOptionalUint32{}.Destroy(r.ClaimedCount)
}

type FfiConverterOsGymWorkspacePoolStatus struct{}

var FfiConverterOsGymWorkspacePoolStatusINSTANCE = FfiConverterOsGymWorkspacePoolStatus{}

func (c FfiConverterOsGymWorkspacePoolStatus) Lift(rb RustBufferI) OsGymWorkspacePoolStatus {
	return LiftFromRustBuffer[OsGymWorkspacePoolStatus](c, rb)
}

func (c FfiConverterOsGymWorkspacePoolStatus) Read(reader io.Reader) OsGymWorkspacePoolStatus {
	return OsGymWorkspacePoolStatus{
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
	}
}

func (c FfiConverterOsGymWorkspacePoolStatus) Lower(value OsGymWorkspacePoolStatus) C.RustBuffer {
	return LowerIntoRustBuffer[OsGymWorkspacePoolStatus](c, value)
}

func (c FfiConverterOsGymWorkspacePoolStatus) LowerExternal(value OsGymWorkspacePoolStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OsGymWorkspacePoolStatus](c, value))
}

func (c FfiConverterOsGymWorkspacePoolStatus) Write(writer io.Writer, value OsGymWorkspacePoolStatus) {
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Phase)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.TotalCount)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.AvailableCount)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.ClaimedCount)
}

type FfiDestroyerOsGymWorkspacePoolStatus struct{}

func (_ FfiDestroyerOsGymWorkspacePoolStatus) Destroy(value OsGymWorkspacePoolStatus) {
	value.Destroy()
}

type OidcConfig struct {
	CredentialsSecret      string
	TokenUrl               string
	AwsRoleArn             *string
	AwsRegion              *string
	RefreshIntervalSeconds *uint32
}

func (r *OidcConfig) Destroy() {
	FfiDestroyerString{}.Destroy(r.CredentialsSecret)
	FfiDestroyerString{}.Destroy(r.TokenUrl)
	FfiDestroyerOptionalString{}.Destroy(r.AwsRoleArn)
	FfiDestroyerOptionalString{}.Destroy(r.AwsRegion)
	FfiDestroyerOptionalUint32{}.Destroy(r.RefreshIntervalSeconds)
}

type FfiConverterOidcConfig struct{}

var FfiConverterOidcConfigINSTANCE = FfiConverterOidcConfig{}

func (c FfiConverterOidcConfig) Lift(rb RustBufferI) OidcConfig {
	return LiftFromRustBuffer[OidcConfig](c, rb)
}

func (c FfiConverterOidcConfig) Read(reader io.Reader) OidcConfig {
	return OidcConfig{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
	}
}

func (c FfiConverterOidcConfig) Lower(value OidcConfig) C.RustBuffer {
	return LowerIntoRustBuffer[OidcConfig](c, value)
}

func (c FfiConverterOidcConfig) LowerExternal(value OidcConfig) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[OidcConfig](c, value))
}

func (c FfiConverterOidcConfig) Write(writer io.Writer, value OidcConfig) {
	FfiConverterStringINSTANCE.Write(writer, value.CredentialsSecret)
	FfiConverterStringINSTANCE.Write(writer, value.TokenUrl)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.AwsRoleArn)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.AwsRegion)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.RefreshIntervalSeconds)
}

type FfiDestroyerOidcConfig struct{}

func (_ FfiDestroyerOidcConfig) Destroy(value OidcConfig) {
	value.Destroy()
}

type PoolSpec struct {
	Replicas    uint32
	Template    PoolTemplate
	Autoscaling *WarmPoolAutoscaling
	Services    *[]SandboxService
}

func (r *PoolSpec) Destroy() {
	FfiDestroyerUint32{}.Destroy(r.Replicas)
	FfiDestroyerPoolTemplate{}.Destroy(r.Template)
	FfiDestroyerOptionalWarmPoolAutoscaling{}.Destroy(r.Autoscaling)
	FfiDestroyerOptionalSequenceSandboxService{}.Destroy(r.Services)
}
func (_self PoolSpec) Eq(other PoolSpec) bool {
	_selfBuf := FfiConverterPoolSpecINSTANCE.Lower(_self)
	return FfiConverterBoolINSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) C.int8_t {
		return C.uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_eq(
			_selfBuf, FfiConverterPoolSpecINSTANCE.Lower(other), _uniffiStatus)
	}))
}
func (_self PoolSpec) Ne(other PoolSpec) bool {
	_selfBuf := FfiConverterPoolSpecINSTANCE.Lower(_self)
	return FfiConverterBoolINSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) C.int8_t {
		return C.uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_eq_ne(
			_selfBuf, FfiConverterPoolSpecINSTANCE.Lower(other), _uniffiStatus)
	}))
}
func (_self PoolSpec) Hash() uint64 {
	_selfBuf := FfiConverterPoolSpecINSTANCE.Lower(_self)
	return FfiConverterUint64INSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_cyclops_sdk_schema_fn_method_poolspec_uniffi_trait_hash(
			_selfBuf, _uniffiStatus)
	}))
}

type FfiConverterPoolSpec struct{}

var FfiConverterPoolSpecINSTANCE = FfiConverterPoolSpec{}

func (c FfiConverterPoolSpec) Lift(rb RustBufferI) PoolSpec {
	return LiftFromRustBuffer[PoolSpec](c, rb)
}

func (c FfiConverterPoolSpec) Read(reader io.Reader) PoolSpec {
	return PoolSpec{
		FfiConverterUint32INSTANCE.Read(reader),
		FfiConverterPoolTemplateINSTANCE.Read(reader),
		FfiConverterOptionalWarmPoolAutoscalingINSTANCE.Read(reader),
		FfiConverterOptionalSequenceSandboxServiceINSTANCE.Read(reader),
	}
}

func (c FfiConverterPoolSpec) Lower(value PoolSpec) C.RustBuffer {
	return LowerIntoRustBuffer[PoolSpec](c, value)
}

func (c FfiConverterPoolSpec) LowerExternal(value PoolSpec) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[PoolSpec](c, value))
}

func (c FfiConverterPoolSpec) Write(writer io.Writer, value PoolSpec) {
	FfiConverterUint32INSTANCE.Write(writer, value.Replicas)
	FfiConverterPoolTemplateINSTANCE.Write(writer, value.Template)
	FfiConverterOptionalWarmPoolAutoscalingINSTANCE.Write(writer, value.Autoscaling)
	FfiConverterOptionalSequenceSandboxServiceINSTANCE.Write(writer, value.Services)
}

type FfiDestroyerPoolSpec struct{}

func (_ FfiDestroyerPoolSpec) Destroy(value PoolSpec) {
	value.Destroy()
}

type PoolTemplate struct {
	Runtime            *RuntimeKind
	RuntimeClassName   *string
	NodeSelector       *map[string]string
	Tolerations        *[]*PreservedJson
	Command            *[]string
	ContainerDiskImage string
	ImagePullSecret    *string
	CpuCores           *uint32
	Memory             *string
	Firmware           *Firmware
	Probes             **PreservedJson
	Oidc               *OidcConfig
}

func (r *PoolTemplate) Destroy() {
	FfiDestroyerOptionalRuntimeKind{}.Destroy(r.Runtime)
	FfiDestroyerOptionalString{}.Destroy(r.RuntimeClassName)
	FfiDestroyerOptionalMapStringString{}.Destroy(r.NodeSelector)
	FfiDestroyerOptionalSequencePreservedJson{}.Destroy(r.Tolerations)
	FfiDestroyerOptionalSequenceString{}.Destroy(r.Command)
	FfiDestroyerString{}.Destroy(r.ContainerDiskImage)
	FfiDestroyerOptionalString{}.Destroy(r.ImagePullSecret)
	FfiDestroyerOptionalUint32{}.Destroy(r.CpuCores)
	FfiDestroyerOptionalString{}.Destroy(r.Memory)
	FfiDestroyerOptionalFirmware{}.Destroy(r.Firmware)
	FfiDestroyerOptionalPreservedJson{}.Destroy(r.Probes)
	FfiDestroyerOptionalOidcConfig{}.Destroy(r.Oidc)
}

type FfiConverterPoolTemplate struct{}

var FfiConverterPoolTemplateINSTANCE = FfiConverterPoolTemplate{}

func (c FfiConverterPoolTemplate) Lift(rb RustBufferI) PoolTemplate {
	return LiftFromRustBuffer[PoolTemplate](c, rb)
}

func (c FfiConverterPoolTemplate) Read(reader io.Reader) PoolTemplate {
	return PoolTemplate{
		FfiConverterOptionalRuntimeKindINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalMapStringStringINSTANCE.Read(reader),
		FfiConverterOptionalSequencePreservedJsonINSTANCE.Read(reader),
		FfiConverterOptionalSequenceStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalFirmwareINSTANCE.Read(reader),
		FfiConverterOptionalPreservedJsonINSTANCE.Read(reader),
		FfiConverterOptionalOidcConfigINSTANCE.Read(reader),
	}
}

func (c FfiConverterPoolTemplate) Lower(value PoolTemplate) C.RustBuffer {
	return LowerIntoRustBuffer[PoolTemplate](c, value)
}

func (c FfiConverterPoolTemplate) LowerExternal(value PoolTemplate) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[PoolTemplate](c, value))
}

func (c FfiConverterPoolTemplate) Write(writer io.Writer, value PoolTemplate) {
	FfiConverterOptionalRuntimeKindINSTANCE.Write(writer, value.Runtime)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.RuntimeClassName)
	FfiConverterOptionalMapStringStringINSTANCE.Write(writer, value.NodeSelector)
	FfiConverterOptionalSequencePreservedJsonINSTANCE.Write(writer, value.Tolerations)
	FfiConverterOptionalSequenceStringINSTANCE.Write(writer, value.Command)
	FfiConverterStringINSTANCE.Write(writer, value.ContainerDiskImage)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.ImagePullSecret)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.CpuCores)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Memory)
	FfiConverterOptionalFirmwareINSTANCE.Write(writer, value.Firmware)
	FfiConverterOptionalPreservedJsonINSTANCE.Write(writer, value.Probes)
	FfiConverterOptionalOidcConfigINSTANCE.Write(writer, value.Oidc)
}

type FfiDestroyerPoolTemplate struct{}

func (_ FfiDestroyerPoolTemplate) Destroy(value PoolTemplate) {
	value.Destroy()
}

type SandboxService struct {
	Name       string
	TargetPort uint16
	Protocol   *ServiceProtocol
}

func (r *SandboxService) Destroy() {
	FfiDestroyerString{}.Destroy(r.Name)
	FfiDestroyerUint16{}.Destroy(r.TargetPort)
	FfiDestroyerOptionalServiceProtocol{}.Destroy(r.Protocol)
}

type FfiConverterSandboxService struct{}

var FfiConverterSandboxServiceINSTANCE = FfiConverterSandboxService{}

func (c FfiConverterSandboxService) Lift(rb RustBufferI) SandboxService {
	return LiftFromRustBuffer[SandboxService](c, rb)
}

func (c FfiConverterSandboxService) Read(reader io.Reader) SandboxService {
	return SandboxService{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterUint16INSTANCE.Read(reader),
		FfiConverterOptionalServiceProtocolINSTANCE.Read(reader),
	}
}

func (c FfiConverterSandboxService) Lower(value SandboxService) C.RustBuffer {
	return LowerIntoRustBuffer[SandboxService](c, value)
}

func (c FfiConverterSandboxService) LowerExternal(value SandboxService) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[SandboxService](c, value))
}

func (c FfiConverterSandboxService) Write(writer io.Writer, value SandboxService) {
	FfiConverterStringINSTANCE.Write(writer, value.Name)
	FfiConverterUint16INSTANCE.Write(writer, value.TargetPort)
	FfiConverterOptionalServiceProtocolINSTANCE.Write(writer, value.Protocol)
}

type FfiDestroyerSandboxService struct{}

func (_ FfiDestroyerSandboxService) Destroy(value SandboxService) {
	value.Destroy()
}

type SandboxTemplateRef struct {
	Name string
}

func (r *SandboxTemplateRef) Destroy() {
	FfiDestroyerString{}.Destroy(r.Name)
}

type FfiConverterSandboxTemplateRef struct{}

var FfiConverterSandboxTemplateRefINSTANCE = FfiConverterSandboxTemplateRef{}

func (c FfiConverterSandboxTemplateRef) Lift(rb RustBufferI) SandboxTemplateRef {
	return LiftFromRustBuffer[SandboxTemplateRef](c, rb)
}

func (c FfiConverterSandboxTemplateRef) Read(reader io.Reader) SandboxTemplateRef {
	return SandboxTemplateRef{
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterSandboxTemplateRef) Lower(value SandboxTemplateRef) C.RustBuffer {
	return LowerIntoRustBuffer[SandboxTemplateRef](c, value)
}

func (c FfiConverterSandboxTemplateRef) LowerExternal(value SandboxTemplateRef) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[SandboxTemplateRef](c, value))
}

func (c FfiConverterSandboxTemplateRef) Write(writer io.Writer, value SandboxTemplateRef) {
	FfiConverterStringINSTANCE.Write(writer, value.Name)
}

type FfiDestroyerSandboxTemplateRef struct{}

func (_ FfiDestroyerSandboxTemplateRef) Destroy(value SandboxTemplateRef) {
	value.Destroy()
}

type VmTemplate struct {
	ContainerDiskImage string
	Command            *[]string
	Runtime            *RuntimeKind
	RuntimeClassName   *string
	NodeSelector       *map[string]string
	Tolerations        *[]*PreservedJson
	ImagePullPolicy    *ImagePullPolicy
	ImagePullSecret    *string
	CpuCores           *uint32
	Memory             *string
	Firmware           *Firmware
	Probes             **PreservedJson
	Services           *[]SandboxService
	Oidc               *OidcConfig
}

func (r *VmTemplate) Destroy() {
	FfiDestroyerString{}.Destroy(r.ContainerDiskImage)
	FfiDestroyerOptionalSequenceString{}.Destroy(r.Command)
	FfiDestroyerOptionalRuntimeKind{}.Destroy(r.Runtime)
	FfiDestroyerOptionalString{}.Destroy(r.RuntimeClassName)
	FfiDestroyerOptionalMapStringString{}.Destroy(r.NodeSelector)
	FfiDestroyerOptionalSequencePreservedJson{}.Destroy(r.Tolerations)
	FfiDestroyerOptionalImagePullPolicy{}.Destroy(r.ImagePullPolicy)
	FfiDestroyerOptionalString{}.Destroy(r.ImagePullSecret)
	FfiDestroyerOptionalUint32{}.Destroy(r.CpuCores)
	FfiDestroyerOptionalString{}.Destroy(r.Memory)
	FfiDestroyerOptionalFirmware{}.Destroy(r.Firmware)
	FfiDestroyerOptionalPreservedJson{}.Destroy(r.Probes)
	FfiDestroyerOptionalSequenceSandboxService{}.Destroy(r.Services)
	FfiDestroyerOptionalOidcConfig{}.Destroy(r.Oidc)
}

type FfiConverterVmTemplate struct{}

var FfiConverterVmTemplateINSTANCE = FfiConverterVmTemplate{}

func (c FfiConverterVmTemplate) Lift(rb RustBufferI) VmTemplate {
	return LiftFromRustBuffer[VmTemplate](c, rb)
}

func (c FfiConverterVmTemplate) Read(reader io.Reader) VmTemplate {
	return VmTemplate{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterOptionalSequenceStringINSTANCE.Read(reader),
		FfiConverterOptionalRuntimeKindINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalMapStringStringINSTANCE.Read(reader),
		FfiConverterOptionalSequencePreservedJsonINSTANCE.Read(reader),
		FfiConverterOptionalImagePullPolicyINSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalStringINSTANCE.Read(reader),
		FfiConverterOptionalFirmwareINSTANCE.Read(reader),
		FfiConverterOptionalPreservedJsonINSTANCE.Read(reader),
		FfiConverterOptionalSequenceSandboxServiceINSTANCE.Read(reader),
		FfiConverterOptionalOidcConfigINSTANCE.Read(reader),
	}
}

func (c FfiConverterVmTemplate) Lower(value VmTemplate) C.RustBuffer {
	return LowerIntoRustBuffer[VmTemplate](c, value)
}

func (c FfiConverterVmTemplate) LowerExternal(value VmTemplate) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[VmTemplate](c, value))
}

func (c FfiConverterVmTemplate) Write(writer io.Writer, value VmTemplate) {
	FfiConverterStringINSTANCE.Write(writer, value.ContainerDiskImage)
	FfiConverterOptionalSequenceStringINSTANCE.Write(writer, value.Command)
	FfiConverterOptionalRuntimeKindINSTANCE.Write(writer, value.Runtime)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.RuntimeClassName)
	FfiConverterOptionalMapStringStringINSTANCE.Write(writer, value.NodeSelector)
	FfiConverterOptionalSequencePreservedJsonINSTANCE.Write(writer, value.Tolerations)
	FfiConverterOptionalImagePullPolicyINSTANCE.Write(writer, value.ImagePullPolicy)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.ImagePullSecret)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.CpuCores)
	FfiConverterOptionalStringINSTANCE.Write(writer, value.Memory)
	FfiConverterOptionalFirmwareINSTANCE.Write(writer, value.Firmware)
	FfiConverterOptionalPreservedJsonINSTANCE.Write(writer, value.Probes)
	FfiConverterOptionalSequenceSandboxServiceINSTANCE.Write(writer, value.Services)
	FfiConverterOptionalOidcConfigINSTANCE.Write(writer, value.Oidc)
}

type FfiDestroyerVmTemplate struct{}

func (_ FfiDestroyerVmTemplate) Destroy(value VmTemplate) {
	value.Destroy()
}

type WarmPoolAutoscaling struct {
	MinPoolSize     *uint32
	InitialPoolSize *uint32
	MaxPoolSize     *uint32
}

func (r *WarmPoolAutoscaling) Destroy() {
	FfiDestroyerOptionalUint32{}.Destroy(r.MinPoolSize)
	FfiDestroyerOptionalUint32{}.Destroy(r.InitialPoolSize)
	FfiDestroyerOptionalUint32{}.Destroy(r.MaxPoolSize)
}

type FfiConverterWarmPoolAutoscaling struct{}

var FfiConverterWarmPoolAutoscalingINSTANCE = FfiConverterWarmPoolAutoscaling{}

func (c FfiConverterWarmPoolAutoscaling) Lift(rb RustBufferI) WarmPoolAutoscaling {
	return LiftFromRustBuffer[WarmPoolAutoscaling](c, rb)
}

func (c FfiConverterWarmPoolAutoscaling) Read(reader io.Reader) WarmPoolAutoscaling {
	return WarmPoolAutoscaling{
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
		FfiConverterOptionalUint32INSTANCE.Read(reader),
	}
}

func (c FfiConverterWarmPoolAutoscaling) Lower(value WarmPoolAutoscaling) C.RustBuffer {
	return LowerIntoRustBuffer[WarmPoolAutoscaling](c, value)
}

func (c FfiConverterWarmPoolAutoscaling) LowerExternal(value WarmPoolAutoscaling) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[WarmPoolAutoscaling](c, value))
}

func (c FfiConverterWarmPoolAutoscaling) Write(writer io.Writer, value WarmPoolAutoscaling) {
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.MinPoolSize)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.InitialPoolSize)
	FfiConverterOptionalUint32INSTANCE.Write(writer, value.MaxPoolSize)
}

type FfiDestroyerWarmPoolAutoscaling struct{}

func (_ FfiDestroyerWarmPoolAutoscaling) Destroy(value WarmPoolAutoscaling) {
	value.Destroy()
}

type Firmware uint

const (
	FirmwareBios Firmware = 1
	FirmwareEfi  Firmware = 2
)

type FfiConverterFirmware struct{}

var FfiConverterFirmwareINSTANCE = FfiConverterFirmware{}

func (c FfiConverterFirmware) Lift(rb RustBufferI) Firmware {
	return LiftFromRustBuffer[Firmware](c, rb)
}

func (c FfiConverterFirmware) Lower(value Firmware) C.RustBuffer {
	return LowerIntoRustBuffer[Firmware](c, value)
}

func (c FfiConverterFirmware) LowerExternal(value Firmware) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[Firmware](c, value))
}
func (FfiConverterFirmware) Read(reader io.Reader) Firmware {
	id := readInt32(reader)
	return Firmware(id)
}

func (FfiConverterFirmware) Write(writer io.Writer, value Firmware) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerFirmware struct{}

func (_ FfiDestroyerFirmware) Destroy(value Firmware) {
}

type ImagePullPolicy uint

const (
	ImagePullPolicyAlways       ImagePullPolicy = 1
	ImagePullPolicyIfNotPresent ImagePullPolicy = 2
	ImagePullPolicyNever        ImagePullPolicy = 3
)

type FfiConverterImagePullPolicy struct{}

var FfiConverterImagePullPolicyINSTANCE = FfiConverterImagePullPolicy{}

func (c FfiConverterImagePullPolicy) Lift(rb RustBufferI) ImagePullPolicy {
	return LiftFromRustBuffer[ImagePullPolicy](c, rb)
}

func (c FfiConverterImagePullPolicy) Lower(value ImagePullPolicy) C.RustBuffer {
	return LowerIntoRustBuffer[ImagePullPolicy](c, value)
}

func (c FfiConverterImagePullPolicy) LowerExternal(value ImagePullPolicy) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ImagePullPolicy](c, value))
}
func (FfiConverterImagePullPolicy) Read(reader io.Reader) ImagePullPolicy {
	id := readInt32(reader)
	return ImagePullPolicy(id)
}

func (FfiConverterImagePullPolicy) Write(writer io.Writer, value ImagePullPolicy) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerImagePullPolicy struct{}

func (_ FfiDestroyerImagePullPolicy) Destroy(value ImagePullPolicy) {
}

type JsonValueError struct {
	err error
}

// Convenience method to turn *JsonValueError into error
// Avoiding treating nil pointer as non nil error interface
func (err *JsonValueError) AsError() error {
	if err == nil {
		return nil
	} else {
		return err
	}
}

func (err JsonValueError) Error() string {
	return fmt.Sprintf("JsonValueError: %s", err.err.Error())
}

func (err JsonValueError) Unwrap() error {
	return err.err
}

// Err* are used for checking error type with `errors.Is`
var ErrJsonValueErrorInvalid = fmt.Errorf("JsonValueErrorInvalid")

// Variant structs
type JsonValueErrorInvalid struct {
	Reason string
}

func NewJsonValueErrorInvalid(
	reason string,
) *JsonValueError {
	return &JsonValueError{err: &JsonValueErrorInvalid{
		Reason: reason}}
}

func (e JsonValueErrorInvalid) destroy() {
	FfiDestroyerString{}.Destroy(e.Reason)
}

func (err JsonValueErrorInvalid) Error() string {
	return fmt.Sprint("Invalid",
		": ",

		"Reason=",
		err.Reason,
	)
}

func (self JsonValueErrorInvalid) Is(target error) bool {
	return target == ErrJsonValueErrorInvalid
}

type FfiConverterJsonValueError struct{}

var FfiConverterJsonValueErrorINSTANCE = FfiConverterJsonValueError{}

func (c FfiConverterJsonValueError) Lift(eb RustBufferI) *JsonValueError {
	return LiftFromRustBuffer[*JsonValueError](c, eb)
}

func (c FfiConverterJsonValueError) Lower(value *JsonValueError) C.RustBuffer {
	return LowerIntoRustBuffer[*JsonValueError](c, value)
}

func (c FfiConverterJsonValueError) LowerExternal(value *JsonValueError) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*JsonValueError](c, value))
}

func (c FfiConverterJsonValueError) Read(reader io.Reader) *JsonValueError {
	errorID := readUint32(reader)

	switch errorID {
	case 1:
		return &JsonValueError{&JsonValueErrorInvalid{
			Reason: FfiConverterStringINSTANCE.Read(reader),
		}}
	default:
		panic(fmt.Sprintf("Unknown error code %d in FfiConverterJsonValueError.Read()", errorID))
	}
}

func (c FfiConverterJsonValueError) Write(writer io.Writer, value *JsonValueError) {
	switch variantValue := value.err.(type) {
	case *JsonValueErrorInvalid:
		writeInt32(writer, 1)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Reason)
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiConverterJsonValueError.Write", value))
	}
}

type FfiDestroyerJsonValueError struct{}

func (_ FfiDestroyerJsonValueError) Destroy(value *JsonValueError) {
	switch variantValue := value.err.(type) {
	case JsonValueErrorInvalid:
		variantValue.destroy()
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiDestroyerJsonValueError.Destroy", value))
	}
}

type RuntimeKind uint

const (
	RuntimeKindKubevirt RuntimeKind = 1
	RuntimeKindMacos    RuntimeKind = 2
	RuntimeKindGvisor   RuntimeKind = 3
)

type FfiConverterRuntimeKind struct{}

var FfiConverterRuntimeKindINSTANCE = FfiConverterRuntimeKind{}

func (c FfiConverterRuntimeKind) Lift(rb RustBufferI) RuntimeKind {
	return LiftFromRustBuffer[RuntimeKind](c, rb)
}

func (c FfiConverterRuntimeKind) Lower(value RuntimeKind) C.RustBuffer {
	return LowerIntoRustBuffer[RuntimeKind](c, value)
}

func (c FfiConverterRuntimeKind) LowerExternal(value RuntimeKind) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[RuntimeKind](c, value))
}
func (FfiConverterRuntimeKind) Read(reader io.Reader) RuntimeKind {
	id := readInt32(reader)
	return RuntimeKind(id)
}

func (FfiConverterRuntimeKind) Write(writer io.Writer, value RuntimeKind) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerRuntimeKind struct{}

func (_ FfiDestroyerRuntimeKind) Destroy(value RuntimeKind) {
}

type ServiceProtocol uint

const (
	ServiceProtocolTcp ServiceProtocol = 1
	ServiceProtocolUdp ServiceProtocol = 2
)

type FfiConverterServiceProtocol struct{}

var FfiConverterServiceProtocolINSTANCE = FfiConverterServiceProtocol{}

func (c FfiConverterServiceProtocol) Lift(rb RustBufferI) ServiceProtocol {
	return LiftFromRustBuffer[ServiceProtocol](c, rb)
}

func (c FfiConverterServiceProtocol) Lower(value ServiceProtocol) C.RustBuffer {
	return LowerIntoRustBuffer[ServiceProtocol](c, value)
}

func (c FfiConverterServiceProtocol) LowerExternal(value ServiceProtocol) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ServiceProtocol](c, value))
}
func (FfiConverterServiceProtocol) Read(reader io.Reader) ServiceProtocol {
	id := readInt32(reader)
	return ServiceProtocol(id)
}

func (FfiConverterServiceProtocol) Write(writer io.Writer, value ServiceProtocol) {
	writeInt32(writer, int32(value))
}

type FfiDestroyerServiceProtocol struct{}

func (_ FfiDestroyerServiceProtocol) Destroy(value ServiceProtocol) {
}

type FfiConverterOptionalUint32 struct{}

var FfiConverterOptionalUint32INSTANCE = FfiConverterOptionalUint32{}

func (c FfiConverterOptionalUint32) Lift(rb RustBufferI) *uint32 {
	return LiftFromRustBuffer[*uint32](c, rb)
}

func (_ FfiConverterOptionalUint32) Read(reader io.Reader) *uint32 {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterUint32INSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalUint32) Lower(value *uint32) C.RustBuffer {
	return LowerIntoRustBuffer[*uint32](c, value)
}

func (c FfiConverterOptionalUint32) LowerExternal(value *uint32) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*uint32](c, value))
}

func (_ FfiConverterOptionalUint32) Write(writer io.Writer, value *uint32) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterUint32INSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalUint32 struct{}

func (_ FfiDestroyerOptionalUint32) Destroy(value *uint32) {
	if value != nil {
		FfiDestroyerUint32{}.Destroy(*value)
	}
}

type FfiConverterOptionalBool struct{}

var FfiConverterOptionalBoolINSTANCE = FfiConverterOptionalBool{}

func (c FfiConverterOptionalBool) Lift(rb RustBufferI) *bool {
	return LiftFromRustBuffer[*bool](c, rb)
}

func (_ FfiConverterOptionalBool) Read(reader io.Reader) *bool {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBoolINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBool) Lower(value *bool) C.RustBuffer {
	return LowerIntoRustBuffer[*bool](c, value)
}

func (c FfiConverterOptionalBool) LowerExternal(value *bool) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*bool](c, value))
}

func (_ FfiConverterOptionalBool) Write(writer io.Writer, value *bool) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBoolINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBool struct{}

func (_ FfiDestroyerOptionalBool) Destroy(value *bool) {
	if value != nil {
		FfiDestroyerBool{}.Destroy(*value)
	}
}

type FfiConverterOptionalString struct{}

var FfiConverterOptionalStringINSTANCE = FfiConverterOptionalString{}

func (c FfiConverterOptionalString) Lift(rb RustBufferI) *string {
	return LiftFromRustBuffer[*string](c, rb)
}

func (_ FfiConverterOptionalString) Read(reader io.Reader) *string {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterStringINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalString) Lower(value *string) C.RustBuffer {
	return LowerIntoRustBuffer[*string](c, value)
}

func (c FfiConverterOptionalString) LowerExternal(value *string) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*string](c, value))
}

func (_ FfiConverterOptionalString) Write(writer io.Writer, value *string) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterStringINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalString struct{}

func (_ FfiDestroyerOptionalString) Destroy(value *string) {
	if value != nil {
		FfiDestroyerString{}.Destroy(*value)
	}
}

type FfiConverterOptionalPreservedJson struct{}

var FfiConverterOptionalPreservedJsonINSTANCE = FfiConverterOptionalPreservedJson{}

func (c FfiConverterOptionalPreservedJson) Lift(rb RustBufferI) **PreservedJson {
	return LiftFromRustBuffer[**PreservedJson](c, rb)
}

func (_ FfiConverterOptionalPreservedJson) Read(reader io.Reader) **PreservedJson {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterPreservedJsonINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalPreservedJson) Lower(value **PreservedJson) C.RustBuffer {
	return LowerIntoRustBuffer[**PreservedJson](c, value)
}

func (c FfiConverterOptionalPreservedJson) LowerExternal(value **PreservedJson) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[**PreservedJson](c, value))
}

func (_ FfiConverterOptionalPreservedJson) Write(writer io.Writer, value **PreservedJson) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterPreservedJsonINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalPreservedJson struct{}

func (_ FfiDestroyerOptionalPreservedJson) Destroy(value **PreservedJson) {
	if value != nil {
		FfiDestroyerPreservedJson{}.Destroy(*value)
	}
}

type FfiConverterOptionalClaimLifecycle struct{}

var FfiConverterOptionalClaimLifecycleINSTANCE = FfiConverterOptionalClaimLifecycle{}

func (c FfiConverterOptionalClaimLifecycle) Lift(rb RustBufferI) *ClaimLifecycle {
	return LiftFromRustBuffer[*ClaimLifecycle](c, rb)
}

func (_ FfiConverterOptionalClaimLifecycle) Read(reader io.Reader) *ClaimLifecycle {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterClaimLifecycleINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalClaimLifecycle) Lower(value *ClaimLifecycle) C.RustBuffer {
	return LowerIntoRustBuffer[*ClaimLifecycle](c, value)
}

func (c FfiConverterOptionalClaimLifecycle) LowerExternal(value *ClaimLifecycle) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*ClaimLifecycle](c, value))
}

func (_ FfiConverterOptionalClaimLifecycle) Write(writer io.Writer, value *ClaimLifecycle) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterClaimLifecycleINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalClaimLifecycle struct{}

func (_ FfiDestroyerOptionalClaimLifecycle) Destroy(value *ClaimLifecycle) {
	if value != nil {
		FfiDestroyerClaimLifecycle{}.Destroy(*value)
	}
}

type FfiConverterOptionalOsGymSandboxClaimSandbox struct{}

var FfiConverterOptionalOsGymSandboxClaimSandboxINSTANCE = FfiConverterOptionalOsGymSandboxClaimSandbox{}

func (c FfiConverterOptionalOsGymSandboxClaimSandbox) Lift(rb RustBufferI) *OsGymSandboxClaimSandbox {
	return LiftFromRustBuffer[*OsGymSandboxClaimSandbox](c, rb)
}

func (_ FfiConverterOptionalOsGymSandboxClaimSandbox) Read(reader io.Reader) *OsGymSandboxClaimSandbox {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterOsGymSandboxClaimSandboxINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalOsGymSandboxClaimSandbox) Lower(value *OsGymSandboxClaimSandbox) C.RustBuffer {
	return LowerIntoRustBuffer[*OsGymSandboxClaimSandbox](c, value)
}

func (c FfiConverterOptionalOsGymSandboxClaimSandbox) LowerExternal(value *OsGymSandboxClaimSandbox) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*OsGymSandboxClaimSandbox](c, value))
}

func (_ FfiConverterOptionalOsGymSandboxClaimSandbox) Write(writer io.Writer, value *OsGymSandboxClaimSandbox) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterOsGymSandboxClaimSandboxINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalOsGymSandboxClaimSandbox struct{}

func (_ FfiDestroyerOptionalOsGymSandboxClaimSandbox) Destroy(value *OsGymSandboxClaimSandbox) {
	if value != nil {
		FfiDestroyerOsGymSandboxClaimSandbox{}.Destroy(*value)
	}
}

type FfiConverterOptionalOidcConfig struct{}

var FfiConverterOptionalOidcConfigINSTANCE = FfiConverterOptionalOidcConfig{}

func (c FfiConverterOptionalOidcConfig) Lift(rb RustBufferI) *OidcConfig {
	return LiftFromRustBuffer[*OidcConfig](c, rb)
}

func (_ FfiConverterOptionalOidcConfig) Read(reader io.Reader) *OidcConfig {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterOidcConfigINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalOidcConfig) Lower(value *OidcConfig) C.RustBuffer {
	return LowerIntoRustBuffer[*OidcConfig](c, value)
}

func (c FfiConverterOptionalOidcConfig) LowerExternal(value *OidcConfig) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*OidcConfig](c, value))
}

func (_ FfiConverterOptionalOidcConfig) Write(writer io.Writer, value *OidcConfig) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterOidcConfigINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalOidcConfig struct{}

func (_ FfiDestroyerOptionalOidcConfig) Destroy(value *OidcConfig) {
	if value != nil {
		FfiDestroyerOidcConfig{}.Destroy(*value)
	}
}

type FfiConverterOptionalWarmPoolAutoscaling struct{}

var FfiConverterOptionalWarmPoolAutoscalingINSTANCE = FfiConverterOptionalWarmPoolAutoscaling{}

func (c FfiConverterOptionalWarmPoolAutoscaling) Lift(rb RustBufferI) *WarmPoolAutoscaling {
	return LiftFromRustBuffer[*WarmPoolAutoscaling](c, rb)
}

func (_ FfiConverterOptionalWarmPoolAutoscaling) Read(reader io.Reader) *WarmPoolAutoscaling {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterWarmPoolAutoscalingINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalWarmPoolAutoscaling) Lower(value *WarmPoolAutoscaling) C.RustBuffer {
	return LowerIntoRustBuffer[*WarmPoolAutoscaling](c, value)
}

func (c FfiConverterOptionalWarmPoolAutoscaling) LowerExternal(value *WarmPoolAutoscaling) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*WarmPoolAutoscaling](c, value))
}

func (_ FfiConverterOptionalWarmPoolAutoscaling) Write(writer io.Writer, value *WarmPoolAutoscaling) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterWarmPoolAutoscalingINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalWarmPoolAutoscaling struct{}

func (_ FfiDestroyerOptionalWarmPoolAutoscaling) Destroy(value *WarmPoolAutoscaling) {
	if value != nil {
		FfiDestroyerWarmPoolAutoscaling{}.Destroy(*value)
	}
}

type FfiConverterOptionalFirmware struct{}

var FfiConverterOptionalFirmwareINSTANCE = FfiConverterOptionalFirmware{}

func (c FfiConverterOptionalFirmware) Lift(rb RustBufferI) *Firmware {
	return LiftFromRustBuffer[*Firmware](c, rb)
}

func (_ FfiConverterOptionalFirmware) Read(reader io.Reader) *Firmware {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterFirmwareINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalFirmware) Lower(value *Firmware) C.RustBuffer {
	return LowerIntoRustBuffer[*Firmware](c, value)
}

func (c FfiConverterOptionalFirmware) LowerExternal(value *Firmware) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*Firmware](c, value))
}

func (_ FfiConverterOptionalFirmware) Write(writer io.Writer, value *Firmware) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterFirmwareINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalFirmware struct{}

func (_ FfiDestroyerOptionalFirmware) Destroy(value *Firmware) {
	if value != nil {
		FfiDestroyerFirmware{}.Destroy(*value)
	}
}

type FfiConverterOptionalImagePullPolicy struct{}

var FfiConverterOptionalImagePullPolicyINSTANCE = FfiConverterOptionalImagePullPolicy{}

func (c FfiConverterOptionalImagePullPolicy) Lift(rb RustBufferI) *ImagePullPolicy {
	return LiftFromRustBuffer[*ImagePullPolicy](c, rb)
}

func (_ FfiConverterOptionalImagePullPolicy) Read(reader io.Reader) *ImagePullPolicy {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterImagePullPolicyINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalImagePullPolicy) Lower(value *ImagePullPolicy) C.RustBuffer {
	return LowerIntoRustBuffer[*ImagePullPolicy](c, value)
}

func (c FfiConverterOptionalImagePullPolicy) LowerExternal(value *ImagePullPolicy) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*ImagePullPolicy](c, value))
}

func (_ FfiConverterOptionalImagePullPolicy) Write(writer io.Writer, value *ImagePullPolicy) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterImagePullPolicyINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalImagePullPolicy struct{}

func (_ FfiDestroyerOptionalImagePullPolicy) Destroy(value *ImagePullPolicy) {
	if value != nil {
		FfiDestroyerImagePullPolicy{}.Destroy(*value)
	}
}

type FfiConverterOptionalRuntimeKind struct{}

var FfiConverterOptionalRuntimeKindINSTANCE = FfiConverterOptionalRuntimeKind{}

func (c FfiConverterOptionalRuntimeKind) Lift(rb RustBufferI) *RuntimeKind {
	return LiftFromRustBuffer[*RuntimeKind](c, rb)
}

func (_ FfiConverterOptionalRuntimeKind) Read(reader io.Reader) *RuntimeKind {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterRuntimeKindINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalRuntimeKind) Lower(value *RuntimeKind) C.RustBuffer {
	return LowerIntoRustBuffer[*RuntimeKind](c, value)
}

func (c FfiConverterOptionalRuntimeKind) LowerExternal(value *RuntimeKind) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*RuntimeKind](c, value))
}

func (_ FfiConverterOptionalRuntimeKind) Write(writer io.Writer, value *RuntimeKind) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterRuntimeKindINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalRuntimeKind struct{}

func (_ FfiDestroyerOptionalRuntimeKind) Destroy(value *RuntimeKind) {
	if value != nil {
		FfiDestroyerRuntimeKind{}.Destroy(*value)
	}
}

type FfiConverterOptionalServiceProtocol struct{}

var FfiConverterOptionalServiceProtocolINSTANCE = FfiConverterOptionalServiceProtocol{}

func (c FfiConverterOptionalServiceProtocol) Lift(rb RustBufferI) *ServiceProtocol {
	return LiftFromRustBuffer[*ServiceProtocol](c, rb)
}

func (_ FfiConverterOptionalServiceProtocol) Read(reader io.Reader) *ServiceProtocol {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterServiceProtocolINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalServiceProtocol) Lower(value *ServiceProtocol) C.RustBuffer {
	return LowerIntoRustBuffer[*ServiceProtocol](c, value)
}

func (c FfiConverterOptionalServiceProtocol) LowerExternal(value *ServiceProtocol) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*ServiceProtocol](c, value))
}

func (_ FfiConverterOptionalServiceProtocol) Write(writer io.Writer, value *ServiceProtocol) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterServiceProtocolINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalServiceProtocol struct{}

func (_ FfiDestroyerOptionalServiceProtocol) Destroy(value *ServiceProtocol) {
	if value != nil {
		FfiDestroyerServiceProtocol{}.Destroy(*value)
	}
}

type FfiConverterOptionalSequenceString struct{}

var FfiConverterOptionalSequenceStringINSTANCE = FfiConverterOptionalSequenceString{}

func (c FfiConverterOptionalSequenceString) Lift(rb RustBufferI) *[]string {
	return LiftFromRustBuffer[*[]string](c, rb)
}

func (_ FfiConverterOptionalSequenceString) Read(reader io.Reader) *[]string {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterSequenceStringINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalSequenceString) Lower(value *[]string) C.RustBuffer {
	return LowerIntoRustBuffer[*[]string](c, value)
}

func (c FfiConverterOptionalSequenceString) LowerExternal(value *[]string) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*[]string](c, value))
}

func (_ FfiConverterOptionalSequenceString) Write(writer io.Writer, value *[]string) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterSequenceStringINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalSequenceString struct{}

func (_ FfiDestroyerOptionalSequenceString) Destroy(value *[]string) {
	if value != nil {
		FfiDestroyerSequenceString{}.Destroy(*value)
	}
}

type FfiConverterOptionalSequencePreservedJson struct{}

var FfiConverterOptionalSequencePreservedJsonINSTANCE = FfiConverterOptionalSequencePreservedJson{}

func (c FfiConverterOptionalSequencePreservedJson) Lift(rb RustBufferI) *[]*PreservedJson {
	return LiftFromRustBuffer[*[]*PreservedJson](c, rb)
}

func (_ FfiConverterOptionalSequencePreservedJson) Read(reader io.Reader) *[]*PreservedJson {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterSequencePreservedJsonINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalSequencePreservedJson) Lower(value *[]*PreservedJson) C.RustBuffer {
	return LowerIntoRustBuffer[*[]*PreservedJson](c, value)
}

func (c FfiConverterOptionalSequencePreservedJson) LowerExternal(value *[]*PreservedJson) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*[]*PreservedJson](c, value))
}

func (_ FfiConverterOptionalSequencePreservedJson) Write(writer io.Writer, value *[]*PreservedJson) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterSequencePreservedJsonINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalSequencePreservedJson struct{}

func (_ FfiDestroyerOptionalSequencePreservedJson) Destroy(value *[]*PreservedJson) {
	if value != nil {
		FfiDestroyerSequencePreservedJson{}.Destroy(*value)
	}
}

type FfiConverterOptionalSequenceOsGymSandboxClaimCondition struct{}

var FfiConverterOptionalSequenceOsGymSandboxClaimConditionINSTANCE = FfiConverterOptionalSequenceOsGymSandboxClaimCondition{}

func (c FfiConverterOptionalSequenceOsGymSandboxClaimCondition) Lift(rb RustBufferI) *[]OsGymSandboxClaimCondition {
	return LiftFromRustBuffer[*[]OsGymSandboxClaimCondition](c, rb)
}

func (_ FfiConverterOptionalSequenceOsGymSandboxClaimCondition) Read(reader io.Reader) *[]OsGymSandboxClaimCondition {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterSequenceOsGymSandboxClaimConditionINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalSequenceOsGymSandboxClaimCondition) Lower(value *[]OsGymSandboxClaimCondition) C.RustBuffer {
	return LowerIntoRustBuffer[*[]OsGymSandboxClaimCondition](c, value)
}

func (c FfiConverterOptionalSequenceOsGymSandboxClaimCondition) LowerExternal(value *[]OsGymSandboxClaimCondition) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*[]OsGymSandboxClaimCondition](c, value))
}

func (_ FfiConverterOptionalSequenceOsGymSandboxClaimCondition) Write(writer io.Writer, value *[]OsGymSandboxClaimCondition) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterSequenceOsGymSandboxClaimConditionINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalSequenceOsGymSandboxClaimCondition struct{}

func (_ FfiDestroyerOptionalSequenceOsGymSandboxClaimCondition) Destroy(value *[]OsGymSandboxClaimCondition) {
	if value != nil {
		FfiDestroyerSequenceOsGymSandboxClaimCondition{}.Destroy(*value)
	}
}

type FfiConverterOptionalSequenceSandboxService struct{}

var FfiConverterOptionalSequenceSandboxServiceINSTANCE = FfiConverterOptionalSequenceSandboxService{}

func (c FfiConverterOptionalSequenceSandboxService) Lift(rb RustBufferI) *[]SandboxService {
	return LiftFromRustBuffer[*[]SandboxService](c, rb)
}

func (_ FfiConverterOptionalSequenceSandboxService) Read(reader io.Reader) *[]SandboxService {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterSequenceSandboxServiceINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalSequenceSandboxService) Lower(value *[]SandboxService) C.RustBuffer {
	return LowerIntoRustBuffer[*[]SandboxService](c, value)
}

func (c FfiConverterOptionalSequenceSandboxService) LowerExternal(value *[]SandboxService) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*[]SandboxService](c, value))
}

func (_ FfiConverterOptionalSequenceSandboxService) Write(writer io.Writer, value *[]SandboxService) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterSequenceSandboxServiceINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalSequenceSandboxService struct{}

func (_ FfiDestroyerOptionalSequenceSandboxService) Destroy(value *[]SandboxService) {
	if value != nil {
		FfiDestroyerSequenceSandboxService{}.Destroy(*value)
	}
}

type FfiConverterOptionalMapStringString struct{}

var FfiConverterOptionalMapStringStringINSTANCE = FfiConverterOptionalMapStringString{}

func (c FfiConverterOptionalMapStringString) Lift(rb RustBufferI) *map[string]string {
	return LiftFromRustBuffer[*map[string]string](c, rb)
}

func (_ FfiConverterOptionalMapStringString) Read(reader io.Reader) *map[string]string {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterMapStringStringINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalMapStringString) Lower(value *map[string]string) C.RustBuffer {
	return LowerIntoRustBuffer[*map[string]string](c, value)
}

func (c FfiConverterOptionalMapStringString) LowerExternal(value *map[string]string) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*map[string]string](c, value))
}

func (_ FfiConverterOptionalMapStringString) Write(writer io.Writer, value *map[string]string) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterMapStringStringINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalMapStringString struct{}

func (_ FfiDestroyerOptionalMapStringString) Destroy(value *map[string]string) {
	if value != nil {
		FfiDestroyerMapStringString{}.Destroy(*value)
	}
}

type FfiConverterSequenceString struct{}

var FfiConverterSequenceStringINSTANCE = FfiConverterSequenceString{}

func (c FfiConverterSequenceString) Lift(rb RustBufferI) []string {
	return LiftFromRustBuffer[[]string](c, rb)
}

func (c FfiConverterSequenceString) Read(reader io.Reader) []string {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]string, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterStringINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceString) Lower(value []string) C.RustBuffer {
	return LowerIntoRustBuffer[[]string](c, value)
}

func (c FfiConverterSequenceString) LowerExternal(value []string) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]string](c, value))
}

func (c FfiConverterSequenceString) Write(writer io.Writer, value []string) {
	if len(value) > math.MaxInt32 {
		panic("[]string is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterStringINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceString struct{}

func (FfiDestroyerSequenceString) Destroy(sequence []string) {
	for _, value := range sequence {
		FfiDestroyerString{}.Destroy(value)
	}
}

type FfiConverterSequencePreservedJson struct{}

var FfiConverterSequencePreservedJsonINSTANCE = FfiConverterSequencePreservedJson{}

func (c FfiConverterSequencePreservedJson) Lift(rb RustBufferI) []*PreservedJson {
	return LiftFromRustBuffer[[]*PreservedJson](c, rb)
}

func (c FfiConverterSequencePreservedJson) Read(reader io.Reader) []*PreservedJson {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]*PreservedJson, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterPreservedJsonINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequencePreservedJson) Lower(value []*PreservedJson) C.RustBuffer {
	return LowerIntoRustBuffer[[]*PreservedJson](c, value)
}

func (c FfiConverterSequencePreservedJson) LowerExternal(value []*PreservedJson) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]*PreservedJson](c, value))
}

func (c FfiConverterSequencePreservedJson) Write(writer io.Writer, value []*PreservedJson) {
	if len(value) > math.MaxInt32 {
		panic("[]*PreservedJson is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterPreservedJsonINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequencePreservedJson struct{}

func (FfiDestroyerSequencePreservedJson) Destroy(sequence []*PreservedJson) {
	for _, value := range sequence {
		FfiDestroyerPreservedJson{}.Destroy(value)
	}
}

type FfiConverterSequenceOsGymSandboxClaimCondition struct{}

var FfiConverterSequenceOsGymSandboxClaimConditionINSTANCE = FfiConverterSequenceOsGymSandboxClaimCondition{}

func (c FfiConverterSequenceOsGymSandboxClaimCondition) Lift(rb RustBufferI) []OsGymSandboxClaimCondition {
	return LiftFromRustBuffer[[]OsGymSandboxClaimCondition](c, rb)
}

func (c FfiConverterSequenceOsGymSandboxClaimCondition) Read(reader io.Reader) []OsGymSandboxClaimCondition {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]OsGymSandboxClaimCondition, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterOsGymSandboxClaimConditionINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceOsGymSandboxClaimCondition) Lower(value []OsGymSandboxClaimCondition) C.RustBuffer {
	return LowerIntoRustBuffer[[]OsGymSandboxClaimCondition](c, value)
}

func (c FfiConverterSequenceOsGymSandboxClaimCondition) LowerExternal(value []OsGymSandboxClaimCondition) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]OsGymSandboxClaimCondition](c, value))
}

func (c FfiConverterSequenceOsGymSandboxClaimCondition) Write(writer io.Writer, value []OsGymSandboxClaimCondition) {
	if len(value) > math.MaxInt32 {
		panic("[]OsGymSandboxClaimCondition is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterOsGymSandboxClaimConditionINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceOsGymSandboxClaimCondition struct{}

func (FfiDestroyerSequenceOsGymSandboxClaimCondition) Destroy(sequence []OsGymSandboxClaimCondition) {
	for _, value := range sequence {
		FfiDestroyerOsGymSandboxClaimCondition{}.Destroy(value)
	}
}

type FfiConverterSequenceSandboxService struct{}

var FfiConverterSequenceSandboxServiceINSTANCE = FfiConverterSequenceSandboxService{}

func (c FfiConverterSequenceSandboxService) Lift(rb RustBufferI) []SandboxService {
	return LiftFromRustBuffer[[]SandboxService](c, rb)
}

func (c FfiConverterSequenceSandboxService) Read(reader io.Reader) []SandboxService {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]SandboxService, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterSandboxServiceINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceSandboxService) Lower(value []SandboxService) C.RustBuffer {
	return LowerIntoRustBuffer[[]SandboxService](c, value)
}

func (c FfiConverterSequenceSandboxService) LowerExternal(value []SandboxService) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]SandboxService](c, value))
}

func (c FfiConverterSequenceSandboxService) Write(writer io.Writer, value []SandboxService) {
	if len(value) > math.MaxInt32 {
		panic("[]SandboxService is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterSandboxServiceINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceSandboxService struct{}

func (FfiDestroyerSequenceSandboxService) Destroy(sequence []SandboxService) {
	for _, value := range sequence {
		FfiDestroyerSandboxService{}.Destroy(value)
	}
}

type FfiConverterMapStringString struct{}

var FfiConverterMapStringStringINSTANCE = FfiConverterMapStringString{}

func (c FfiConverterMapStringString) Lift(rb RustBufferI) map[string]string {
	return LiftFromRustBuffer[map[string]string](c, rb)
}

func (_ FfiConverterMapStringString) Read(reader io.Reader) map[string]string {
	result := make(map[string]string)
	length := readInt32(reader)
	for i := int32(0); i < length; i++ {
		key := FfiConverterStringINSTANCE.Read(reader)
		value := FfiConverterStringINSTANCE.Read(reader)
		result[key] = value
	}
	return result
}

func (c FfiConverterMapStringString) Lower(value map[string]string) C.RustBuffer {
	return LowerIntoRustBuffer[map[string]string](c, value)
}

func (c FfiConverterMapStringString) LowerExternal(value map[string]string) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[map[string]string](c, value))
}

func (_ FfiConverterMapStringString) Write(writer io.Writer, mapValue map[string]string) {
	if len(mapValue) > math.MaxInt32 {
		panic("map[string]string is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(mapValue)))
	for key, value := range mapValue {
		FfiConverterStringINSTANCE.Write(writer, key)
		FfiConverterStringINSTANCE.Write(writer, value)
	}
}

type FfiDestroyerMapStringString struct{}

func (_ FfiDestroyerMapStringString) Destroy(mapValue map[string]string) {
	for key, value := range mapValue {
		FfiDestroyerString{}.Destroy(key)
		FfiDestroyerString{}.Destroy(value)
	}
}
