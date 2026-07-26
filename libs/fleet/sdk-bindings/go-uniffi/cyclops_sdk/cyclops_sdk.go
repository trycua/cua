package cyclops_sdk

// #include <cyclops_sdk.h>
import "C"

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk_schema"
	"io"
	"math"
	"reflect"
	"runtime"
	"runtime/cgo"
	"sync"
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
		C.ffi_cyclops_sdk_rustbuffer_free(cb.inner, status)
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
		return C.ffi_cyclops_sdk_rustbuffer_from_bytes(foreign, status)
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

	FfiConverterHttpClientINSTANCE.register()
	uniffiCheckChecksums()
}

func uniffiCheckChecksums() {
	// Get the bindings contract version from our ComponentInterface
	bindingsContractVersion := 30
	// Get the scaffolding contract version by calling the into the dylib
	scaffoldingContractVersion := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint32_t {
		return C.ffi_cyclops_sdk_uniffi_contract_version()
	})
	if bindingsContractVersion != int(scaffoldingContractVersion) {
		// If this happens try cleaning and rebuilding your project
		panic("cyclops_sdk: UniFFI contract version mismatch")
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_claim()
		})
		if checksum != 23330 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_claim: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_claim()
		})
		if checksum != 20460 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_claim: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_claim()
		})
		if checksum != 17760 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_claim: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_claims()
		})
		if checksum != 7802 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_claims: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_wait_claim()
		})
		if checksum != 18984 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_wait_claim: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_pool()
		})
		if checksum != 48557 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_create_pool: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_pool()
		})
		if checksum != 31235 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_delete_pool: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_pool()
		})
		if checksum != 43327 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_get_pool: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_pools()
		})
		if checksum != 27984 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_list_pools: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_update_pool()
		})
		if checksum != 17695 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_update_pool: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_cyclopsclient_service_request()
		})
		if checksum != 46699 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_cyclopsclient_service_request: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_method_httpclient_execute()
		})
		if checksum != 38803 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_method_httpclient_execute: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect()
		})
		if checksum != 54404 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_constructor_cyclopsclient_connect: UniFFI API checksum mismatch")
		}
	}
	{
		checksum := rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint16_t {
			return C.uniffi_cyclops_sdk_checksum_constructor_cyclopscredentials_new()
		})
		if checksum != 25746 {
			// If this happens try cleaning and rebuilding your project
			panic("cyclops_sdk: uniffi_cyclops_sdk_checksum_constructor_cyclopscredentials_new: UniFFI API checksum mismatch")
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

type FfiConverterBytes struct{}

var FfiConverterBytesINSTANCE = FfiConverterBytes{}

func (c FfiConverterBytes) Lower(value []byte) C.RustBuffer {
	return LowerIntoRustBuffer[[]byte](c, value)
}

func (c FfiConverterBytes) LowerExternal(value []byte) ExternalCRustBuffer {
	return RustBufferFromC(c.Lower(value))
}

func (c FfiConverterBytes) Write(writer io.Writer, value []byte) {
	if len(value) > math.MaxInt32 {
		panic("[]byte is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	write_length, err := writer.Write(value)
	if err != nil {
		panic(err)
	}
	if write_length != len(value) {
		panic(fmt.Errorf("bad write length when writing []byte, expected %d, written %d", len(value), write_length))
	}
}

func (c FfiConverterBytes) Lift(rb RustBufferI) []byte {
	return LiftFromRustBuffer[[]byte](c, rb)
}

func (c FfiConverterBytes) Read(reader io.Reader) []byte {
	length := readInt32(reader)
	buffer := make([]byte, length)
	read_length, err := reader.Read(buffer)
	if err != nil && err != io.EOF {
		panic(err)
	}
	if read_length != int(length) {
		panic(fmt.Errorf("bad read length when reading []byte, expected %d, read %d", length, read_length))
	}
	return buffer
}

type FfiDestroyerBytes struct{}

func (FfiDestroyerBytes) Destroy(_ []byte) {}

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

type CyclopsClientInterface interface {
	CreateClaim(request CreateClaimRequest) (Claim, error)
	DeleteClaim(claim Claim) error
	GetClaim(claim Claim) (Claim, error)
	ListClaims(namespace string) ([]Claim, error)
	WaitClaim(claim Claim) (Sandbox, error)
	CreatePool(request CreatePoolRequest) (Pool, error)
	DeletePool(pool Pool) error
	GetPool(pool Pool) (Pool, error)
	ListPools(namespace string) ([]Pool, error)
	UpdatePool(pool Pool) (Pool, error)
	ServiceRequest(sandbox Sandbox, service string, path string, request HttpRequest) (HttpResponse, error)
}
type CyclopsClient struct {
	ffiObject FfiObject
}

func CyclopsClientConnect(configuration CyclopsConfiguration, httpClient HttpClient) (*CyclopsClient, error) {
	_uniffiRV, _uniffiErr := rustCallWithError[*SdkError](FfiConverterSdkError{}, func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_cyclops_sdk_fn_constructor_cyclopsclient_connect(FfiConverterCyclopsConfigurationINSTANCE.Lower(configuration), FfiConverterHttpClientINSTANCE.Lower(httpClient), _uniffiStatus)
	})
	if _uniffiErr != nil {
		var _uniffiDefaultValue *CyclopsClient
		return _uniffiDefaultValue, _uniffiErr
	} else {
		return FfiConverterCyclopsClientINSTANCE.Lift(_uniffiRV), nil
	}
}

func (_self *CyclopsClient) CreateClaim(request CreateClaimRequest) (Claim, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) Claim {
			return FfiConverterClaimINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_create_claim(
			_pointer, FfiConverterCreateClaimRequestINSTANCE.Lower(request)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) DeleteClaim(claim Claim) error {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	_, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) struct{} {
			C.ffi_cyclops_sdk_rust_future_complete_void(handle, status)
			return struct{}{}
		},
		// liftFn
		func(_ struct{}) struct{} { return struct{}{} },
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_claim(
			_pointer, FfiConverterClaimINSTANCE.Lower(claim)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_void(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_void(handle)
		},
	)

	if err == nil {
		return nil
	}

	return err
}

func (_self *CyclopsClient) GetClaim(claim Claim) (Claim, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) Claim {
			return FfiConverterClaimINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_get_claim(
			_pointer, FfiConverterClaimINSTANCE.Lower(claim)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) ListClaims(namespace string) ([]Claim, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) []Claim {
			return FfiConverterSequenceClaimINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_list_claims(
			_pointer, FfiConverterStringINSTANCE.Lower(namespace)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) WaitClaim(claim Claim) (Sandbox, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) Sandbox {
			return FfiConverterSandboxINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_wait_claim(
			_pointer, FfiConverterClaimINSTANCE.Lower(claim)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) CreatePool(request CreatePoolRequest) (Pool, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) Pool {
			return FfiConverterPoolINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_create_pool(
			_pointer, FfiConverterCreatePoolRequestINSTANCE.Lower(request)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) DeletePool(pool Pool) error {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	_, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) struct{} {
			C.ffi_cyclops_sdk_rust_future_complete_void(handle, status)
			return struct{}{}
		},
		// liftFn
		func(_ struct{}) struct{} { return struct{}{} },
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_delete_pool(
			_pointer, FfiConverterPoolINSTANCE.Lower(pool)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_void(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_void(handle)
		},
	)

	if err == nil {
		return nil
	}

	return err
}

func (_self *CyclopsClient) GetPool(pool Pool) (Pool, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) Pool {
			return FfiConverterPoolINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_get_pool(
			_pointer, FfiConverterPoolINSTANCE.Lower(pool)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) ListPools(namespace string) ([]Pool, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) []Pool {
			return FfiConverterSequencePoolINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_list_pools(
			_pointer, FfiConverterStringINSTANCE.Lower(namespace)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) UpdatePool(pool Pool) (Pool, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) Pool {
			return FfiConverterPoolINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_update_pool(
			_pointer, FfiConverterPoolINSTANCE.Lower(pool)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}

func (_self *CyclopsClient) ServiceRequest(sandbox Sandbox, service string, path string, request HttpRequest) (HttpResponse, error) {
	_pointer := _self.ffiObject.incrementPointer("*CyclopsClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*SdkError](
		FfiConverterSdkErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) HttpResponse {
			return FfiConverterHttpResponseINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_cyclopsclient_service_request(
			_pointer, FfiConverterSandboxINSTANCE.Lower(sandbox), FfiConverterStringINSTANCE.Lower(service), FfiConverterStringINSTANCE.Lower(path), FfiConverterHttpRequestINSTANCE.Lower(request)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}
func (object *CyclopsClient) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterCyclopsClient struct{}

var FfiConverterCyclopsClientINSTANCE = FfiConverterCyclopsClient{}

func (c FfiConverterCyclopsClient) Lift(handle C.uint64_t) *CyclopsClient {
	result := &CyclopsClient{
		newFfiObject(
			handle,
			func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
				return C.uniffi_cyclops_sdk_fn_clone_cyclopsclient(handle, status)
			},
			func(handle C.uint64_t, status *C.RustCallStatus) {
				C.uniffi_cyclops_sdk_fn_free_cyclopsclient(handle, status)
			},
		),
	}
	runtime.SetFinalizer(result, (*CyclopsClient).Destroy)
	return result
}

func (c FfiConverterCyclopsClient) Read(reader io.Reader) *CyclopsClient {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterCyclopsClient) Lower(value *CyclopsClient) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	handle := value.ffiObject.incrementPointer("*CyclopsClient")
	defer value.ffiObject.decrementPointer()
	return handle
}

func (c FfiConverterCyclopsClient) Write(writer io.Writer, value *CyclopsClient) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalCyclopsClient(handle uint64) *CyclopsClient {
	return FfiConverterCyclopsClientINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalCyclopsClient(value *CyclopsClient) uint64 {
	return uint64(FfiConverterCyclopsClientINSTANCE.Lower(value))
}

type FfiDestroyerCyclopsClient struct{}

func (_ FfiDestroyerCyclopsClient) Destroy(value *CyclopsClient) {
	value.Destroy()
}

type CyclopsCredentialsInterface interface {
}
type CyclopsCredentials struct {
	ffiObject FfiObject
}

func NewCyclopsCredentials(clientId string, clientSecret string) *CyclopsCredentials {
	return FfiConverterCyclopsCredentialsINSTANCE.Lift(rustCall(func(_uniffiStatus *C.RustCallStatus) C.uint64_t {
		return C.uniffi_cyclops_sdk_fn_constructor_cyclopscredentials_new(FfiConverterStringINSTANCE.Lower(clientId), FfiConverterStringINSTANCE.Lower(clientSecret), _uniffiStatus)
	}))
}

func (object *CyclopsCredentials) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterCyclopsCredentials struct{}

var FfiConverterCyclopsCredentialsINSTANCE = FfiConverterCyclopsCredentials{}

func (c FfiConverterCyclopsCredentials) Lift(handle C.uint64_t) *CyclopsCredentials {
	result := &CyclopsCredentials{
		newFfiObject(
			handle,
			func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
				return C.uniffi_cyclops_sdk_fn_clone_cyclopscredentials(handle, status)
			},
			func(handle C.uint64_t, status *C.RustCallStatus) {
				C.uniffi_cyclops_sdk_fn_free_cyclopscredentials(handle, status)
			},
		),
	}
	runtime.SetFinalizer(result, (*CyclopsCredentials).Destroy)
	return result
}

func (c FfiConverterCyclopsCredentials) Read(reader io.Reader) *CyclopsCredentials {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterCyclopsCredentials) Lower(value *CyclopsCredentials) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	handle := value.ffiObject.incrementPointer("*CyclopsCredentials")
	defer value.ffiObject.decrementPointer()
	return handle
}

func (c FfiConverterCyclopsCredentials) Write(writer io.Writer, value *CyclopsCredentials) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalCyclopsCredentials(handle uint64) *CyclopsCredentials {
	return FfiConverterCyclopsCredentialsINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalCyclopsCredentials(value *CyclopsCredentials) uint64 {
	return uint64(FfiConverterCyclopsCredentialsINSTANCE.Lower(value))
}

type FfiDestroyerCyclopsCredentials struct{}

func (_ FfiDestroyerCyclopsCredentials) Destroy(value *CyclopsCredentials) {
	value.Destroy()
}

type HttpClient interface {
	Execute(request HttpRequest) (HttpResponse, error)
}
type HttpClientImpl struct {
	ffiObject FfiObject
}

func (_self *HttpClientImpl) Execute(request HttpRequest) (HttpResponse, error) {
	_pointer := _self.ffiObject.incrementPointer("HttpClient")
	defer _self.ffiObject.decrementPointer()
	res, err := uniffiRustCallAsync[*HttpError](
		FfiConverterHttpErrorINSTANCE,
		// completeFn
		func(handle C.uint64_t, status *C.RustCallStatus) RustBufferI {
			res := C.ffi_cyclops_sdk_rust_future_complete_rust_buffer(handle, status)
			return GoRustBuffer{
				inner: res,
			}
		},
		// liftFn
		func(ffi RustBufferI) HttpResponse {
			return FfiConverterHttpResponseINSTANCE.Lift(ffi)
		},
		C.uniffi_cyclops_sdk_fn_method_httpclient_execute(
			_pointer, FfiConverterHttpRequestINSTANCE.Lower(request)),
		// pollFn
		func(handle C.uint64_t, continuation C.UniffiRustFutureContinuationCallback, data C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_poll_rust_buffer(handle, continuation, data)
		},
		// freeFn
		func(handle C.uint64_t) {
			C.ffi_cyclops_sdk_rust_future_free_rust_buffer(handle)
		},
	)

	if err == nil {
		return res, nil
	}

	return res, err
}
func (object *HttpClientImpl) Destroy() {
	runtime.SetFinalizer(object, nil)
	object.ffiObject.destroy()
}

type FfiConverterHttpClient struct {
	handleMap *concurrentHandleMap[HttpClient]
}

var FfiConverterHttpClientINSTANCE = FfiConverterHttpClient{
	handleMap: newConcurrentHandleMap[HttpClient](),
}

func (c FfiConverterHttpClient) Lift(handle C.uint64_t) HttpClient {
	if uint64(handle)&1 == 0 {
		// Rust-generated handle (even), construct a new object wrapping the handle
		result := &HttpClientImpl{
			newFfiObject(
				handle,
				func(handle C.uint64_t, status *C.RustCallStatus) C.uint64_t {
					return C.uniffi_cyclops_sdk_fn_clone_httpclient(handle, status)
				},
				func(handle C.uint64_t, status *C.RustCallStatus) {
					C.uniffi_cyclops_sdk_fn_free_httpclient(handle, status)
				},
			),
		}
		runtime.SetFinalizer(result, (*HttpClientImpl).Destroy)
		return result
	} else {
		// Go-generated handle (odd), retrieve from the handle map
		val, ok := c.handleMap.tryGet(uint64(handle))
		if !ok {
			panic(fmt.Errorf("no callback in handle map: %d", handle))
		}
		c.handleMap.remove(uint64(handle))
		return val
	}
}

func (c FfiConverterHttpClient) Read(reader io.Reader) HttpClient {
	return c.Lift(C.uint64_t(readUint64(reader)))
}

func (c FfiConverterHttpClient) Lower(value HttpClient) C.uint64_t {
	// TODO: this is bad - all synchronization from ObjectRuntime.go is discarded here,
	// because the handle will be decremented immediately after this function returns,
	// and someone will be left holding onto a non-locked handle.
	if val, ok := value.(*HttpClientImpl); ok {
		// Rust-backed object, clone the handle
		handle := val.ffiObject.incrementPointer("HttpClient")
		defer val.ffiObject.decrementPointer()
		return handle
	} else {
		// Go-backed object, insert into handle map
		return C.uint64_t(c.handleMap.insert(value))
	}
}

func (c FfiConverterHttpClient) Write(writer io.Writer, value HttpClient) {
	writeUint64(writer, uint64(c.Lower(value)))
}

func LiftFromExternalHttpClient(handle uint64) HttpClient {
	return FfiConverterHttpClientINSTANCE.Lift(C.uint64_t(handle))
}

func LowerToExternalHttpClient(value HttpClient) uint64 {
	return uint64(FfiConverterHttpClientINSTANCE.Lower(value))
}

type FfiDestroyerHttpClient struct{}

func (_ FfiDestroyerHttpClient) Destroy(value HttpClient) {
	if val, ok := value.(*HttpClientImpl); ok {
		val.Destroy()
	}
}

type uniffiCallbackResult C.int8_t

const (
	uniffiIdxCallbackFree               uniffiCallbackResult = 0
	uniffiCallbackResultSuccess         uniffiCallbackResult = 0
	uniffiCallbackResultError           uniffiCallbackResult = 1
	uniffiCallbackUnexpectedResultError uniffiCallbackResult = 2
	uniffiCallbackCancelled             uniffiCallbackResult = 3
)

type concurrentHandleMap[T any] struct {
	handles       map[uint64]T
	currentHandle uint64
	lock          sync.RWMutex
}

func newConcurrentHandleMap[T any]() *concurrentHandleMap[T] {
	return &concurrentHandleMap[T]{
		handles:       map[uint64]T{},
		currentHandle: 1,
	}
}

func (cm *concurrentHandleMap[T]) insert(obj T) uint64 {
	cm.lock.Lock()
	defer cm.lock.Unlock()

	handle := cm.currentHandle
	cm.currentHandle = cm.currentHandle + 2
	cm.handles[handle] = obj
	return handle
}

func (cm *concurrentHandleMap[T]) remove(handle uint64) {
	cm.lock.Lock()
	defer cm.lock.Unlock()

	delete(cm.handles, handle)
}

func (cm *concurrentHandleMap[T]) tryGet(handle uint64) (T, bool) {
	cm.lock.RLock()
	defer cm.lock.RUnlock()

	val, ok := cm.handles[handle]
	return val, ok
}

//export cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientMethod0
func cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientMethod0(uniffiHandle C.uint64_t, request C.RustBuffer, uniffiFutureCallback C.UniffiForeignFutureCompleteRustBuffer, uniffiCallbackData C.uint64_t, uniffiOutDroppedCallback *C.UniffiForeignFutureDroppedCallbackStruct) {
	handle := uint64(uniffiHandle)
	uniffiObj, ok := FfiConverterHttpClientINSTANCE.handleMap.tryGet(handle)
	if !ok {
		panic(fmt.Errorf("no callback in handle map: %d", handle))
	}

	result := make(chan C.UniffiForeignFutureResultRustBuffer, 1)
	cancel := make(chan struct{}, 1)
	guardHandle := cgo.NewHandle(cancel)
	*uniffiOutDroppedCallback = C.UniffiForeignFutureDroppedCallbackStruct{
		handle: C.uint64_t(guardHandle),
		free:   C.UniffiForeignFutureDroppedCallback(C.cyclops_sdk_uniffiFreeGorutine),
	}

	// Wait for compleation or cancel
	go func() {
		select {
		case <-cancel:
		case res := <-result:
			C.call_UniffiForeignFutureCompleteRustBuffer(uniffiFutureCallback, uniffiCallbackData, res)
		}
	}()

	// Eval callback asynchroniously
	go func() {
		asyncResult := &C.UniffiForeignFutureResultRustBuffer{}
		uniffiOutReturn := &asyncResult.returnValue
		callStatus := &asyncResult.callStatus
		defer func() {
			result <- *asyncResult
		}()

		res, err :=
			uniffiObj.Execute(
				FfiConverterHttpRequestINSTANCE.Lift(GoRustBuffer{
					inner: request,
				}),
			)

		if err != nil {
			var actualError *HttpError
			if errors.As(err, &actualError) {
				*callStatus = C.RustCallStatus{
					code:     C.int8_t(uniffiCallbackResultError),
					errorBuf: FfiConverterHttpErrorINSTANCE.Lower(actualError),
				}
			} else {
				*callStatus = C.RustCallStatus{
					code: C.int8_t(uniffiCallbackUnexpectedResultError),
				}
			}
			return
		}

		*uniffiOutReturn = FfiConverterHttpResponseINSTANCE.Lower(res)
	}()
}

var UniffiVTableCallbackInterfaceHttpClientINSTANCE = C.UniffiVTableCallbackInterfaceHttpClient{
	uniffiFree:  (C.UniffiCallbackInterfaceFree)(C.cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientFree),
	uniffiClone: (C.UniffiCallbackInterfaceClone)(C.cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientClone),
	execute:     (C.UniffiCallbackInterfaceHttpClientMethod0)(C.cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientMethod0),
}

//export cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientFree
func cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientFree(handle C.uint64_t) {
	FfiConverterHttpClientINSTANCE.handleMap.remove(uint64(handle))
}

//export cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientClone
func cyclops_sdk_transport_cgo_dispatchCallbackInterfaceHttpClientClone(handle C.uint64_t) C.uint64_t {
	val, ok := FfiConverterHttpClientINSTANCE.handleMap.tryGet(uint64(handle))
	if !ok {
		panic(fmt.Errorf("no callback in handle map: %d", handle))
	}
	return C.uint64_t(FfiConverterHttpClientINSTANCE.handleMap.insert(val))
}

func (c FfiConverterHttpClient) register() {
	C.uniffi_cyclops_sdk_fn_init_callback_vtable_httpclient(&UniffiVTableCallbackInterfaceHttpClientINSTANCE)
}

type Claim struct {
	ApiVersion string
	Kind       string
	Metadata   ResourceMetadata
	Spec       cyclops_sdk_schema.ClaimSpec
	Status     *cyclops_sdk_schema.OsGymSandboxClaimStatus
}

func (r *Claim) Destroy() {
	FfiDestroyerString{}.Destroy(r.ApiVersion)
	FfiDestroyerString{}.Destroy(r.Kind)
	FfiDestroyerResourceMetadata{}.Destroy(r.Metadata)
	cyclops_sdk_schema.FfiDestroyerClaimSpec{}.Destroy(r.Spec)
	FfiDestroyerOptionalOsGymSandboxClaimStatus{}.Destroy(r.Status)
}

type FfiConverterClaim struct{}

var FfiConverterClaimINSTANCE = FfiConverterClaim{}

func (c FfiConverterClaim) Lift(rb RustBufferI) Claim {
	return LiftFromRustBuffer[Claim](c, rb)
}

func (c FfiConverterClaim) Read(reader io.Reader) Claim {
	return Claim{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterResourceMetadataINSTANCE.Read(reader),
		cyclops_sdk_schema.FfiConverterClaimSpecINSTANCE.Read(reader),
		FfiConverterOptionalOsGymSandboxClaimStatusINSTANCE.Read(reader),
	}
}

func (c FfiConverterClaim) Lower(value Claim) C.RustBuffer {
	return LowerIntoRustBuffer[Claim](c, value)
}

func (c FfiConverterClaim) LowerExternal(value Claim) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[Claim](c, value))
}

func (c FfiConverterClaim) Write(writer io.Writer, value Claim) {
	FfiConverterStringINSTANCE.Write(writer, value.ApiVersion)
	FfiConverterStringINSTANCE.Write(writer, value.Kind)
	FfiConverterResourceMetadataINSTANCE.Write(writer, value.Metadata)
	cyclops_sdk_schema.FfiConverterClaimSpecINSTANCE.Write(writer, value.Spec)
	FfiConverterOptionalOsGymSandboxClaimStatusINSTANCE.Write(writer, value.Status)
}

type FfiDestroyerClaim struct{}

func (_ FfiDestroyerClaim) Destroy(value Claim) {
	value.Destroy()
}

type CreateClaimRequest struct {
	Pool Pool
	Spec *cyclops_sdk_schema.ClaimSpec
}

func (r *CreateClaimRequest) Destroy() {
	FfiDestroyerPool{}.Destroy(r.Pool)
	FfiDestroyerOptionalClaimSpec{}.Destroy(r.Spec)
}

type FfiConverterCreateClaimRequest struct{}

var FfiConverterCreateClaimRequestINSTANCE = FfiConverterCreateClaimRequest{}

func (c FfiConverterCreateClaimRequest) Lift(rb RustBufferI) CreateClaimRequest {
	return LiftFromRustBuffer[CreateClaimRequest](c, rb)
}

func (c FfiConverterCreateClaimRequest) Read(reader io.Reader) CreateClaimRequest {
	return CreateClaimRequest{
		FfiConverterPoolINSTANCE.Read(reader),
		FfiConverterOptionalClaimSpecINSTANCE.Read(reader),
	}
}

func (c FfiConverterCreateClaimRequest) Lower(value CreateClaimRequest) C.RustBuffer {
	return LowerIntoRustBuffer[CreateClaimRequest](c, value)
}

func (c FfiConverterCreateClaimRequest) LowerExternal(value CreateClaimRequest) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[CreateClaimRequest](c, value))
}

func (c FfiConverterCreateClaimRequest) Write(writer io.Writer, value CreateClaimRequest) {
	FfiConverterPoolINSTANCE.Write(writer, value.Pool)
	FfiConverterOptionalClaimSpecINSTANCE.Write(writer, value.Spec)
}

type FfiDestroyerCreateClaimRequest struct{}

func (_ FfiDestroyerCreateClaimRequest) Destroy(value CreateClaimRequest) {
	value.Destroy()
}

type CreatePoolRequest struct {
	Namespace string
	Spec      cyclops_sdk_schema.PoolSpec
}

func (r *CreatePoolRequest) Destroy() {
	FfiDestroyerString{}.Destroy(r.Namespace)
	cyclops_sdk_schema.FfiDestroyerPoolSpec{}.Destroy(r.Spec)
}

type FfiConverterCreatePoolRequest struct{}

var FfiConverterCreatePoolRequestINSTANCE = FfiConverterCreatePoolRequest{}

func (c FfiConverterCreatePoolRequest) Lift(rb RustBufferI) CreatePoolRequest {
	return LiftFromRustBuffer[CreatePoolRequest](c, rb)
}

func (c FfiConverterCreatePoolRequest) Read(reader io.Reader) CreatePoolRequest {
	return CreatePoolRequest{
		FfiConverterStringINSTANCE.Read(reader),
		cyclops_sdk_schema.FfiConverterPoolSpecINSTANCE.Read(reader),
	}
}

func (c FfiConverterCreatePoolRequest) Lower(value CreatePoolRequest) C.RustBuffer {
	return LowerIntoRustBuffer[CreatePoolRequest](c, value)
}

func (c FfiConverterCreatePoolRequest) LowerExternal(value CreatePoolRequest) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[CreatePoolRequest](c, value))
}

func (c FfiConverterCreatePoolRequest) Write(writer io.Writer, value CreatePoolRequest) {
	FfiConverterStringINSTANCE.Write(writer, value.Namespace)
	cyclops_sdk_schema.FfiConverterPoolSpecINSTANCE.Write(writer, value.Spec)
}

type FfiDestroyerCreatePoolRequest struct{}

func (_ FfiDestroyerCreatePoolRequest) Destroy(value CreatePoolRequest) {
	value.Destroy()
}

type CyclopsConfiguration struct {
	BaseUrl             string
	TokenUrl            string
	Credentials         *CyclopsCredentials
	PoolPollIntervalMs  uint64
	PoolPollLimit       uint32
	ClaimPollIntervalMs uint64
	ClaimPollLimit      uint32
}

func (r *CyclopsConfiguration) Destroy() {
	FfiDestroyerString{}.Destroy(r.BaseUrl)
	FfiDestroyerString{}.Destroy(r.TokenUrl)
	FfiDestroyerCyclopsCredentials{}.Destroy(r.Credentials)
	FfiDestroyerUint64{}.Destroy(r.PoolPollIntervalMs)
	FfiDestroyerUint32{}.Destroy(r.PoolPollLimit)
	FfiDestroyerUint64{}.Destroy(r.ClaimPollIntervalMs)
	FfiDestroyerUint32{}.Destroy(r.ClaimPollLimit)
}

type FfiConverterCyclopsConfiguration struct{}

var FfiConverterCyclopsConfigurationINSTANCE = FfiConverterCyclopsConfiguration{}

func (c FfiConverterCyclopsConfiguration) Lift(rb RustBufferI) CyclopsConfiguration {
	return LiftFromRustBuffer[CyclopsConfiguration](c, rb)
}

func (c FfiConverterCyclopsConfiguration) Read(reader io.Reader) CyclopsConfiguration {
	return CyclopsConfiguration{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterCyclopsCredentialsINSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint32INSTANCE.Read(reader),
		FfiConverterUint64INSTANCE.Read(reader),
		FfiConverterUint32INSTANCE.Read(reader),
	}
}

func (c FfiConverterCyclopsConfiguration) Lower(value CyclopsConfiguration) C.RustBuffer {
	return LowerIntoRustBuffer[CyclopsConfiguration](c, value)
}

func (c FfiConverterCyclopsConfiguration) LowerExternal(value CyclopsConfiguration) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[CyclopsConfiguration](c, value))
}

func (c FfiConverterCyclopsConfiguration) Write(writer io.Writer, value CyclopsConfiguration) {
	FfiConverterStringINSTANCE.Write(writer, value.BaseUrl)
	FfiConverterStringINSTANCE.Write(writer, value.TokenUrl)
	FfiConverterCyclopsCredentialsINSTANCE.Write(writer, value.Credentials)
	FfiConverterUint64INSTANCE.Write(writer, value.PoolPollIntervalMs)
	FfiConverterUint32INSTANCE.Write(writer, value.PoolPollLimit)
	FfiConverterUint64INSTANCE.Write(writer, value.ClaimPollIntervalMs)
	FfiConverterUint32INSTANCE.Write(writer, value.ClaimPollLimit)
}

type FfiDestroyerCyclopsConfiguration struct{}

func (_ FfiDestroyerCyclopsConfiguration) Destroy(value CyclopsConfiguration) {
	value.Destroy()
}

type HttpHeader struct {
	Name  string
	Value string
}

func (r *HttpHeader) Destroy() {
	FfiDestroyerString{}.Destroy(r.Name)
	FfiDestroyerString{}.Destroy(r.Value)
}

type FfiConverterHttpHeader struct{}

var FfiConverterHttpHeaderINSTANCE = FfiConverterHttpHeader{}

func (c FfiConverterHttpHeader) Lift(rb RustBufferI) HttpHeader {
	return LiftFromRustBuffer[HttpHeader](c, rb)
}

func (c FfiConverterHttpHeader) Read(reader io.Reader) HttpHeader {
	return HttpHeader{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterHttpHeader) Lower(value HttpHeader) C.RustBuffer {
	return LowerIntoRustBuffer[HttpHeader](c, value)
}

func (c FfiConverterHttpHeader) LowerExternal(value HttpHeader) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[HttpHeader](c, value))
}

func (c FfiConverterHttpHeader) Write(writer io.Writer, value HttpHeader) {
	FfiConverterStringINSTANCE.Write(writer, value.Name)
	FfiConverterStringINSTANCE.Write(writer, value.Value)
}

type FfiDestroyerHttpHeader struct{}

func (_ FfiDestroyerHttpHeader) Destroy(value HttpHeader) {
	value.Destroy()
}

type HttpRequest struct {
	Method  string
	Url     string
	Headers []HttpHeader
	Body    *[]byte
}

func (r *HttpRequest) Destroy() {
	FfiDestroyerString{}.Destroy(r.Method)
	FfiDestroyerString{}.Destroy(r.Url)
	FfiDestroyerSequenceHttpHeader{}.Destroy(r.Headers)
	FfiDestroyerOptionalBytes{}.Destroy(r.Body)
}

type FfiConverterHttpRequest struct{}

var FfiConverterHttpRequestINSTANCE = FfiConverterHttpRequest{}

func (c FfiConverterHttpRequest) Lift(rb RustBufferI) HttpRequest {
	return LiftFromRustBuffer[HttpRequest](c, rb)
}

func (c FfiConverterHttpRequest) Read(reader io.Reader) HttpRequest {
	return HttpRequest{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterSequenceHttpHeaderINSTANCE.Read(reader),
		FfiConverterOptionalBytesINSTANCE.Read(reader),
	}
}

func (c FfiConverterHttpRequest) Lower(value HttpRequest) C.RustBuffer {
	return LowerIntoRustBuffer[HttpRequest](c, value)
}

func (c FfiConverterHttpRequest) LowerExternal(value HttpRequest) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[HttpRequest](c, value))
}

func (c FfiConverterHttpRequest) Write(writer io.Writer, value HttpRequest) {
	FfiConverterStringINSTANCE.Write(writer, value.Method)
	FfiConverterStringINSTANCE.Write(writer, value.Url)
	FfiConverterSequenceHttpHeaderINSTANCE.Write(writer, value.Headers)
	FfiConverterOptionalBytesINSTANCE.Write(writer, value.Body)
}

type FfiDestroyerHttpRequest struct{}

func (_ FfiDestroyerHttpRequest) Destroy(value HttpRequest) {
	value.Destroy()
}

type HttpResponse struct {
	Status  uint16
	Headers []HttpHeader
	Body    []byte
}

func (r *HttpResponse) Destroy() {
	FfiDestroyerUint16{}.Destroy(r.Status)
	FfiDestroyerSequenceHttpHeader{}.Destroy(r.Headers)
	FfiDestroyerBytes{}.Destroy(r.Body)
}

type FfiConverterHttpResponse struct{}

var FfiConverterHttpResponseINSTANCE = FfiConverterHttpResponse{}

func (c FfiConverterHttpResponse) Lift(rb RustBufferI) HttpResponse {
	return LiftFromRustBuffer[HttpResponse](c, rb)
}

func (c FfiConverterHttpResponse) Read(reader io.Reader) HttpResponse {
	return HttpResponse{
		FfiConverterUint16INSTANCE.Read(reader),
		FfiConverterSequenceHttpHeaderINSTANCE.Read(reader),
		FfiConverterBytesINSTANCE.Read(reader),
	}
}

func (c FfiConverterHttpResponse) Lower(value HttpResponse) C.RustBuffer {
	return LowerIntoRustBuffer[HttpResponse](c, value)
}

func (c FfiConverterHttpResponse) LowerExternal(value HttpResponse) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[HttpResponse](c, value))
}

func (c FfiConverterHttpResponse) Write(writer io.Writer, value HttpResponse) {
	FfiConverterUint16INSTANCE.Write(writer, value.Status)
	FfiConverterSequenceHttpHeaderINSTANCE.Write(writer, value.Headers)
	FfiConverterBytesINSTANCE.Write(writer, value.Body)
}

type FfiDestroyerHttpResponse struct{}

func (_ FfiDestroyerHttpResponse) Destroy(value HttpResponse) {
	value.Destroy()
}

// UniFFI cannot emit aliases for external record types. Generated bindings use
// `OSGymWorkspacePoolStatus` and `OSGymSandboxClaimStatus` from cyclops_sdk_schema.
type Pool struct {
	ApiVersion string
	Kind       string
	Metadata   ResourceMetadata
	Spec       cyclops_sdk_schema.PoolSpec
	Status     *cyclops_sdk_schema.OsGymWorkspacePoolStatus
}

func (r *Pool) Destroy() {
	FfiDestroyerString{}.Destroy(r.ApiVersion)
	FfiDestroyerString{}.Destroy(r.Kind)
	FfiDestroyerResourceMetadata{}.Destroy(r.Metadata)
	cyclops_sdk_schema.FfiDestroyerPoolSpec{}.Destroy(r.Spec)
	FfiDestroyerOptionalOsGymWorkspacePoolStatus{}.Destroy(r.Status)
}

type FfiConverterPool struct{}

var FfiConverterPoolINSTANCE = FfiConverterPool{}

func (c FfiConverterPool) Lift(rb RustBufferI) Pool {
	return LiftFromRustBuffer[Pool](c, rb)
}

func (c FfiConverterPool) Read(reader io.Reader) Pool {
	return Pool{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterResourceMetadataINSTANCE.Read(reader),
		cyclops_sdk_schema.FfiConverterPoolSpecINSTANCE.Read(reader),
		FfiConverterOptionalOsGymWorkspacePoolStatusINSTANCE.Read(reader),
	}
}

func (c FfiConverterPool) Lower(value Pool) C.RustBuffer {
	return LowerIntoRustBuffer[Pool](c, value)
}

func (c FfiConverterPool) LowerExternal(value Pool) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[Pool](c, value))
}

func (c FfiConverterPool) Write(writer io.Writer, value Pool) {
	FfiConverterStringINSTANCE.Write(writer, value.ApiVersion)
	FfiConverterStringINSTANCE.Write(writer, value.Kind)
	FfiConverterResourceMetadataINSTANCE.Write(writer, value.Metadata)
	cyclops_sdk_schema.FfiConverterPoolSpecINSTANCE.Write(writer, value.Spec)
	FfiConverterOptionalOsGymWorkspacePoolStatusINSTANCE.Write(writer, value.Status)
}

type FfiDestroyerPool struct{}

func (_ FfiDestroyerPool) Destroy(value Pool) {
	value.Destroy()
}

type ResourceMetadata struct {
	Namespace string
	Name      string
	Labels    *map[string]string
}

func (r *ResourceMetadata) Destroy() {
	FfiDestroyerString{}.Destroy(r.Namespace)
	FfiDestroyerString{}.Destroy(r.Name)
	FfiDestroyerOptionalMapStringString{}.Destroy(r.Labels)
}

type FfiConverterResourceMetadata struct{}

var FfiConverterResourceMetadataINSTANCE = FfiConverterResourceMetadata{}

func (c FfiConverterResourceMetadata) Lift(rb RustBufferI) ResourceMetadata {
	return LiftFromRustBuffer[ResourceMetadata](c, rb)
}

func (c FfiConverterResourceMetadata) Read(reader io.Reader) ResourceMetadata {
	return ResourceMetadata{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterOptionalMapStringStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterResourceMetadata) Lower(value ResourceMetadata) C.RustBuffer {
	return LowerIntoRustBuffer[ResourceMetadata](c, value)
}

func (c FfiConverterResourceMetadata) LowerExternal(value ResourceMetadata) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[ResourceMetadata](c, value))
}

func (c FfiConverterResourceMetadata) Write(writer io.Writer, value ResourceMetadata) {
	FfiConverterStringINSTANCE.Write(writer, value.Namespace)
	FfiConverterStringINSTANCE.Write(writer, value.Name)
	FfiConverterOptionalMapStringStringINSTANCE.Write(writer, value.Labels)
}

type FfiDestroyerResourceMetadata struct{}

func (_ FfiDestroyerResourceMetadata) Destroy(value ResourceMetadata) {
	value.Destroy()
}

type Sandbox struct {
	Namespace string
	Claim     string
	Name      string
	Services  []string
}

func (r *Sandbox) Destroy() {
	FfiDestroyerString{}.Destroy(r.Namespace)
	FfiDestroyerString{}.Destroy(r.Claim)
	FfiDestroyerString{}.Destroy(r.Name)
	FfiDestroyerSequenceString{}.Destroy(r.Services)
}

type FfiConverterSandbox struct{}

var FfiConverterSandboxINSTANCE = FfiConverterSandbox{}

func (c FfiConverterSandbox) Lift(rb RustBufferI) Sandbox {
	return LiftFromRustBuffer[Sandbox](c, rb)
}

func (c FfiConverterSandbox) Read(reader io.Reader) Sandbox {
	return Sandbox{
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterStringINSTANCE.Read(reader),
		FfiConverterSequenceStringINSTANCE.Read(reader),
	}
}

func (c FfiConverterSandbox) Lower(value Sandbox) C.RustBuffer {
	return LowerIntoRustBuffer[Sandbox](c, value)
}

func (c FfiConverterSandbox) LowerExternal(value Sandbox) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[Sandbox](c, value))
}

func (c FfiConverterSandbox) Write(writer io.Writer, value Sandbox) {
	FfiConverterStringINSTANCE.Write(writer, value.Namespace)
	FfiConverterStringINSTANCE.Write(writer, value.Claim)
	FfiConverterStringINSTANCE.Write(writer, value.Name)
	FfiConverterSequenceStringINSTANCE.Write(writer, value.Services)
}

type FfiDestroyerSandbox struct{}

func (_ FfiDestroyerSandbox) Destroy(value Sandbox) {
	value.Destroy()
}

type HttpError struct {
	err error
}

// Convenience method to turn *HttpError into error
// Avoiding treating nil pointer as non nil error interface
func (err *HttpError) AsError() error {
	if err == nil {
		return nil
	} else {
		return err
	}
}

func (err HttpError) Error() string {
	return fmt.Sprintf("HttpError: %s", err.err.Error())
}

func (err HttpError) Unwrap() error {
	return err.err
}

// Err* are used for checking error type with `errors.Is`
var ErrHttpErrorTransport = fmt.Errorf("HttpErrorTransport")

// Variant structs
type HttpErrorTransport struct {
	Reason string
}

func NewHttpErrorTransport(
	reason string,
) *HttpError {
	return &HttpError{err: &HttpErrorTransport{
		Reason: reason}}
}

func (e HttpErrorTransport) destroy() {
	FfiDestroyerString{}.Destroy(e.Reason)
}

func (err HttpErrorTransport) Error() string {
	return fmt.Sprint("Transport",
		": ",

		"Reason=",
		err.Reason,
	)
}

func (self HttpErrorTransport) Is(target error) bool {
	return target == ErrHttpErrorTransport
}

type FfiConverterHttpError struct{}

var FfiConverterHttpErrorINSTANCE = FfiConverterHttpError{}

func (c FfiConverterHttpError) Lift(eb RustBufferI) *HttpError {
	return LiftFromRustBuffer[*HttpError](c, eb)
}

func (c FfiConverterHttpError) Lower(value *HttpError) C.RustBuffer {
	return LowerIntoRustBuffer[*HttpError](c, value)
}

func (c FfiConverterHttpError) LowerExternal(value *HttpError) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*HttpError](c, value))
}

func (c FfiConverterHttpError) Read(reader io.Reader) *HttpError {
	errorID := readUint32(reader)

	switch errorID {
	case 1:
		return &HttpError{&HttpErrorTransport{
			Reason: FfiConverterStringINSTANCE.Read(reader),
		}}
	default:
		panic(fmt.Sprintf("Unknown error code %d in FfiConverterHttpError.Read()", errorID))
	}
}

func (c FfiConverterHttpError) Write(writer io.Writer, value *HttpError) {
	switch variantValue := value.err.(type) {
	case *HttpErrorTransport:
		writeInt32(writer, 1)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Reason)
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiConverterHttpError.Write", value))
	}
}

type FfiDestroyerHttpError struct{}

func (_ FfiDestroyerHttpError) Destroy(value *HttpError) {
	switch variantValue := value.err.(type) {
	case HttpErrorTransport:
		variantValue.destroy()
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiDestroyerHttpError.Destroy", value))
	}
}

type SdkError struct {
	err error
}

// Convenience method to turn *SdkError into error
// Avoiding treating nil pointer as non nil error interface
func (err *SdkError) AsError() error {
	if err == nil {
		return nil
	} else {
		return err
	}
}

func (err SdkError) Error() string {
	return fmt.Sprintf("SdkError: %s", err.err.Error())
}

func (err SdkError) Unwrap() error {
	return err.err
}

// Err* are used for checking error type with `errors.Is`
var ErrSdkErrorConfiguration = fmt.Errorf("SdkErrorConfiguration")
var ErrSdkErrorInvalidResourceName = fmt.Errorf("SdkErrorInvalidResourceName")
var ErrSdkErrorTransport = fmt.Errorf("SdkErrorTransport")
var ErrSdkErrorToken = fmt.Errorf("SdkErrorToken")
var ErrSdkErrorBody = fmt.Errorf("SdkErrorBody")
var ErrSdkErrorStatus = fmt.Errorf("SdkErrorStatus")
var ErrSdkErrorUnknownService = fmt.Errorf("SdkErrorUnknownService")
var ErrSdkErrorInvalidServicePath = fmt.Errorf("SdkErrorInvalidServicePath")
var ErrSdkErrorClaimFailed = fmt.Errorf("SdkErrorClaimFailed")
var ErrSdkErrorClaimTimeout = fmt.Errorf("SdkErrorClaimTimeout")

// Variant structs
type SdkErrorConfiguration struct {
	Reason string
}

func NewSdkErrorConfiguration(
	reason string,
) *SdkError {
	return &SdkError{err: &SdkErrorConfiguration{
		Reason: reason}}
}

func (e SdkErrorConfiguration) destroy() {
	FfiDestroyerString{}.Destroy(e.Reason)
}

func (err SdkErrorConfiguration) Error() string {
	return fmt.Sprint("Configuration",
		": ",

		"Reason=",
		err.Reason,
	)
}

func (self SdkErrorConfiguration) Is(target error) bool {
	return target == ErrSdkErrorConfiguration
}

type SdkErrorInvalidResourceName struct {
	Field  string
	Value  string
	Reason string
}

func NewSdkErrorInvalidResourceName(
	field string,
	value string,
	reason string,
) *SdkError {
	return &SdkError{err: &SdkErrorInvalidResourceName{
		Field:  field,
		Value:  value,
		Reason: reason}}
}

func (e SdkErrorInvalidResourceName) destroy() {
	FfiDestroyerString{}.Destroy(e.Field)
	FfiDestroyerString{}.Destroy(e.Value)
	FfiDestroyerString{}.Destroy(e.Reason)
}

func (err SdkErrorInvalidResourceName) Error() string {
	return fmt.Sprint("InvalidResourceName",
		": ",

		"Field=",
		err.Field,
		", ",
		"Value=",
		err.Value,
		", ",
		"Reason=",
		err.Reason,
	)
}

func (self SdkErrorInvalidResourceName) Is(target error) bool {
	return target == ErrSdkErrorInvalidResourceName
}

type SdkErrorTransport struct {
	Reason string
}

func NewSdkErrorTransport(
	reason string,
) *SdkError {
	return &SdkError{err: &SdkErrorTransport{
		Reason: reason}}
}

func (e SdkErrorTransport) destroy() {
	FfiDestroyerString{}.Destroy(e.Reason)
}

func (err SdkErrorTransport) Error() string {
	return fmt.Sprint("Transport",
		": ",

		"Reason=",
		err.Reason,
	)
}

func (self SdkErrorTransport) Is(target error) bool {
	return target == ErrSdkErrorTransport
}

type SdkErrorToken struct {
	Reason string
}

func NewSdkErrorToken(
	reason string,
) *SdkError {
	return &SdkError{err: &SdkErrorToken{
		Reason: reason}}
}

func (e SdkErrorToken) destroy() {
	FfiDestroyerString{}.Destroy(e.Reason)
}

func (err SdkErrorToken) Error() string {
	return fmt.Sprint("Token",
		": ",

		"Reason=",
		err.Reason,
	)
}

func (self SdkErrorToken) Is(target error) bool {
	return target == ErrSdkErrorToken
}

type SdkErrorBody struct {
	Reason string
}

func NewSdkErrorBody(
	reason string,
) *SdkError {
	return &SdkError{err: &SdkErrorBody{
		Reason: reason}}
}

func (e SdkErrorBody) destroy() {
	FfiDestroyerString{}.Destroy(e.Reason)
}

func (err SdkErrorBody) Error() string {
	return fmt.Sprint("Body",
		": ",

		"Reason=",
		err.Reason,
	)
}

func (self SdkErrorBody) Is(target error) bool {
	return target == ErrSdkErrorBody
}

type SdkErrorStatus struct {
	Operation string
	Status    uint16
	Body      string
}

func NewSdkErrorStatus(
	operation string,
	status uint16,
	body string,
) *SdkError {
	return &SdkError{err: &SdkErrorStatus{
		Operation: operation,
		Status:    status,
		Body:      body}}
}

func (e SdkErrorStatus) destroy() {
	FfiDestroyerString{}.Destroy(e.Operation)
	FfiDestroyerUint16{}.Destroy(e.Status)
	FfiDestroyerString{}.Destroy(e.Body)
}

func (err SdkErrorStatus) Error() string {
	return fmt.Sprint("Status",
		": ",

		"Operation=",
		err.Operation,
		", ",
		"Status=",
		err.Status,
		", ",
		"Body=",
		err.Body,
	)
}

func (self SdkErrorStatus) Is(target error) bool {
	return target == ErrSdkErrorStatus
}

type SdkErrorUnknownService struct {
	Requested string
	Available []string
}

func NewSdkErrorUnknownService(
	requested string,
	available []string,
) *SdkError {
	return &SdkError{err: &SdkErrorUnknownService{
		Requested: requested,
		Available: available}}
}

func (e SdkErrorUnknownService) destroy() {
	FfiDestroyerString{}.Destroy(e.Requested)
	FfiDestroyerSequenceString{}.Destroy(e.Available)
}

func (err SdkErrorUnknownService) Error() string {
	return fmt.Sprint("UnknownService",
		": ",

		"Requested=",
		err.Requested,
		", ",
		"Available=",
		err.Available,
	)
}

func (self SdkErrorUnknownService) Is(target error) bool {
	return target == ErrSdkErrorUnknownService
}

type SdkErrorInvalidServicePath struct {
	Path string
}

func NewSdkErrorInvalidServicePath(
	path string,
) *SdkError {
	return &SdkError{err: &SdkErrorInvalidServicePath{
		Path: path}}
}

func (e SdkErrorInvalidServicePath) destroy() {
	FfiDestroyerString{}.Destroy(e.Path)
}

func (err SdkErrorInvalidServicePath) Error() string {
	return fmt.Sprint("InvalidServicePath",
		": ",

		"Path=",
		err.Path,
	)
}

func (self SdkErrorInvalidServicePath) Is(target error) bool {
	return target == ErrSdkErrorInvalidServicePath
}

type SdkErrorClaimFailed struct {
	Phase  string
	Status string
}

func NewSdkErrorClaimFailed(
	phase string,
	status string,
) *SdkError {
	return &SdkError{err: &SdkErrorClaimFailed{
		Phase:  phase,
		Status: status}}
}

func (e SdkErrorClaimFailed) destroy() {
	FfiDestroyerString{}.Destroy(e.Phase)
	FfiDestroyerString{}.Destroy(e.Status)
}

func (err SdkErrorClaimFailed) Error() string {
	return fmt.Sprint("ClaimFailed",
		": ",

		"Phase=",
		err.Phase,
		", ",
		"Status=",
		err.Status,
	)
}

func (self SdkErrorClaimFailed) Is(target error) bool {
	return target == ErrSdkErrorClaimFailed
}

type SdkErrorClaimTimeout struct {
}

func NewSdkErrorClaimTimeout() *SdkError {
	return &SdkError{err: &SdkErrorClaimTimeout{}}
}

func (e SdkErrorClaimTimeout) destroy() {
}

func (err SdkErrorClaimTimeout) Error() string {
	return fmt.Sprint("ClaimTimeout")
}

func (self SdkErrorClaimTimeout) Is(target error) bool {
	return target == ErrSdkErrorClaimTimeout
}

type FfiConverterSdkError struct{}

var FfiConverterSdkErrorINSTANCE = FfiConverterSdkError{}

func (c FfiConverterSdkError) Lift(eb RustBufferI) *SdkError {
	return LiftFromRustBuffer[*SdkError](c, eb)
}

func (c FfiConverterSdkError) Lower(value *SdkError) C.RustBuffer {
	return LowerIntoRustBuffer[*SdkError](c, value)
}

func (c FfiConverterSdkError) LowerExternal(value *SdkError) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*SdkError](c, value))
}

func (c FfiConverterSdkError) Read(reader io.Reader) *SdkError {
	errorID := readUint32(reader)

	switch errorID {
	case 1:
		return &SdkError{&SdkErrorConfiguration{
			Reason: FfiConverterStringINSTANCE.Read(reader),
		}}
	case 2:
		return &SdkError{&SdkErrorInvalidResourceName{
			Field:  FfiConverterStringINSTANCE.Read(reader),
			Value:  FfiConverterStringINSTANCE.Read(reader),
			Reason: FfiConverterStringINSTANCE.Read(reader),
		}}
	case 3:
		return &SdkError{&SdkErrorTransport{
			Reason: FfiConverterStringINSTANCE.Read(reader),
		}}
	case 4:
		return &SdkError{&SdkErrorToken{
			Reason: FfiConverterStringINSTANCE.Read(reader),
		}}
	case 5:
		return &SdkError{&SdkErrorBody{
			Reason: FfiConverterStringINSTANCE.Read(reader),
		}}
	case 6:
		return &SdkError{&SdkErrorStatus{
			Operation: FfiConverterStringINSTANCE.Read(reader),
			Status:    FfiConverterUint16INSTANCE.Read(reader),
			Body:      FfiConverterStringINSTANCE.Read(reader),
		}}
	case 7:
		return &SdkError{&SdkErrorUnknownService{
			Requested: FfiConverterStringINSTANCE.Read(reader),
			Available: FfiConverterSequenceStringINSTANCE.Read(reader),
		}}
	case 8:
		return &SdkError{&SdkErrorInvalidServicePath{
			Path: FfiConverterStringINSTANCE.Read(reader),
		}}
	case 9:
		return &SdkError{&SdkErrorClaimFailed{
			Phase:  FfiConverterStringINSTANCE.Read(reader),
			Status: FfiConverterStringINSTANCE.Read(reader),
		}}
	case 10:
		return &SdkError{&SdkErrorClaimTimeout{}}
	default:
		panic(fmt.Sprintf("Unknown error code %d in FfiConverterSdkError.Read()", errorID))
	}
}

func (c FfiConverterSdkError) Write(writer io.Writer, value *SdkError) {
	switch variantValue := value.err.(type) {
	case *SdkErrorConfiguration:
		writeInt32(writer, 1)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Reason)
	case *SdkErrorInvalidResourceName:
		writeInt32(writer, 2)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Field)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Value)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Reason)
	case *SdkErrorTransport:
		writeInt32(writer, 3)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Reason)
	case *SdkErrorToken:
		writeInt32(writer, 4)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Reason)
	case *SdkErrorBody:
		writeInt32(writer, 5)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Reason)
	case *SdkErrorStatus:
		writeInt32(writer, 6)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Operation)
		FfiConverterUint16INSTANCE.Write(writer, variantValue.Status)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Body)
	case *SdkErrorUnknownService:
		writeInt32(writer, 7)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Requested)
		FfiConverterSequenceStringINSTANCE.Write(writer, variantValue.Available)
	case *SdkErrorInvalidServicePath:
		writeInt32(writer, 8)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Path)
	case *SdkErrorClaimFailed:
		writeInt32(writer, 9)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Phase)
		FfiConverterStringINSTANCE.Write(writer, variantValue.Status)
	case *SdkErrorClaimTimeout:
		writeInt32(writer, 10)
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiConverterSdkError.Write", value))
	}
}

type FfiDestroyerSdkError struct{}

func (_ FfiDestroyerSdkError) Destroy(value *SdkError) {
	switch variantValue := value.err.(type) {
	case SdkErrorConfiguration:
		variantValue.destroy()
	case SdkErrorInvalidResourceName:
		variantValue.destroy()
	case SdkErrorTransport:
		variantValue.destroy()
	case SdkErrorToken:
		variantValue.destroy()
	case SdkErrorBody:
		variantValue.destroy()
	case SdkErrorStatus:
		variantValue.destroy()
	case SdkErrorUnknownService:
		variantValue.destroy()
	case SdkErrorInvalidServicePath:
		variantValue.destroy()
	case SdkErrorClaimFailed:
		variantValue.destroy()
	case SdkErrorClaimTimeout:
		variantValue.destroy()
	default:
		_ = variantValue
		panic(fmt.Sprintf("invalid error value `%v` in FfiDestroyerSdkError.Destroy", value))
	}
}

type FfiConverterOptionalBytes struct{}

var FfiConverterOptionalBytesINSTANCE = FfiConverterOptionalBytes{}

func (c FfiConverterOptionalBytes) Lift(rb RustBufferI) *[]byte {
	return LiftFromRustBuffer[*[]byte](c, rb)
}

func (_ FfiConverterOptionalBytes) Read(reader io.Reader) *[]byte {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := FfiConverterBytesINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalBytes) Lower(value *[]byte) C.RustBuffer {
	return LowerIntoRustBuffer[*[]byte](c, value)
}

func (c FfiConverterOptionalBytes) LowerExternal(value *[]byte) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*[]byte](c, value))
}

func (_ FfiConverterOptionalBytes) Write(writer io.Writer, value *[]byte) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		FfiConverterBytesINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalBytes struct{}

func (_ FfiDestroyerOptionalBytes) Destroy(value *[]byte) {
	if value != nil {
		FfiDestroyerBytes{}.Destroy(*value)
	}
}

type FfiConverterOptionalClaimSpec struct{}

var FfiConverterOptionalClaimSpecINSTANCE = FfiConverterOptionalClaimSpec{}

func (c FfiConverterOptionalClaimSpec) Lift(rb RustBufferI) *cyclops_sdk_schema.ClaimSpec {
	return LiftFromRustBuffer[*cyclops_sdk_schema.ClaimSpec](c, rb)
}

func (_ FfiConverterOptionalClaimSpec) Read(reader io.Reader) *cyclops_sdk_schema.ClaimSpec {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := cyclops_sdk_schema.FfiConverterClaimSpecINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalClaimSpec) Lower(value *cyclops_sdk_schema.ClaimSpec) C.RustBuffer {
	return LowerIntoRustBuffer[*cyclops_sdk_schema.ClaimSpec](c, value)
}

func (c FfiConverterOptionalClaimSpec) LowerExternal(value *cyclops_sdk_schema.ClaimSpec) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*cyclops_sdk_schema.ClaimSpec](c, value))
}

func (_ FfiConverterOptionalClaimSpec) Write(writer io.Writer, value *cyclops_sdk_schema.ClaimSpec) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		cyclops_sdk_schema.FfiConverterClaimSpecINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalClaimSpec struct{}

func (_ FfiDestroyerOptionalClaimSpec) Destroy(value *cyclops_sdk_schema.ClaimSpec) {
	if value != nil {
		cyclops_sdk_schema.FfiDestroyerClaimSpec{}.Destroy(*value)
	}
}

type FfiConverterOptionalOsGymSandboxClaimStatus struct{}

var FfiConverterOptionalOsGymSandboxClaimStatusINSTANCE = FfiConverterOptionalOsGymSandboxClaimStatus{}

func (c FfiConverterOptionalOsGymSandboxClaimStatus) Lift(rb RustBufferI) *cyclops_sdk_schema.OsGymSandboxClaimStatus {
	return LiftFromRustBuffer[*cyclops_sdk_schema.OsGymSandboxClaimStatus](c, rb)
}

func (_ FfiConverterOptionalOsGymSandboxClaimStatus) Read(reader io.Reader) *cyclops_sdk_schema.OsGymSandboxClaimStatus {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := cyclops_sdk_schema.FfiConverterOsGymSandboxClaimStatusINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalOsGymSandboxClaimStatus) Lower(value *cyclops_sdk_schema.OsGymSandboxClaimStatus) C.RustBuffer {
	return LowerIntoRustBuffer[*cyclops_sdk_schema.OsGymSandboxClaimStatus](c, value)
}

func (c FfiConverterOptionalOsGymSandboxClaimStatus) LowerExternal(value *cyclops_sdk_schema.OsGymSandboxClaimStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*cyclops_sdk_schema.OsGymSandboxClaimStatus](c, value))
}

func (_ FfiConverterOptionalOsGymSandboxClaimStatus) Write(writer io.Writer, value *cyclops_sdk_schema.OsGymSandboxClaimStatus) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		cyclops_sdk_schema.FfiConverterOsGymSandboxClaimStatusINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalOsGymSandboxClaimStatus struct{}

func (_ FfiDestroyerOptionalOsGymSandboxClaimStatus) Destroy(value *cyclops_sdk_schema.OsGymSandboxClaimStatus) {
	if value != nil {
		cyclops_sdk_schema.FfiDestroyerOsGymSandboxClaimStatus{}.Destroy(*value)
	}
}

type FfiConverterOptionalOsGymWorkspacePoolStatus struct{}

var FfiConverterOptionalOsGymWorkspacePoolStatusINSTANCE = FfiConverterOptionalOsGymWorkspacePoolStatus{}

func (c FfiConverterOptionalOsGymWorkspacePoolStatus) Lift(rb RustBufferI) *cyclops_sdk_schema.OsGymWorkspacePoolStatus {
	return LiftFromRustBuffer[*cyclops_sdk_schema.OsGymWorkspacePoolStatus](c, rb)
}

func (_ FfiConverterOptionalOsGymWorkspacePoolStatus) Read(reader io.Reader) *cyclops_sdk_schema.OsGymWorkspacePoolStatus {
	if readInt8(reader) == 0 {
		return nil
	}
	temp := cyclops_sdk_schema.FfiConverterOsGymWorkspacePoolStatusINSTANCE.Read(reader)
	return &temp
}

func (c FfiConverterOptionalOsGymWorkspacePoolStatus) Lower(value *cyclops_sdk_schema.OsGymWorkspacePoolStatus) C.RustBuffer {
	return LowerIntoRustBuffer[*cyclops_sdk_schema.OsGymWorkspacePoolStatus](c, value)
}

func (c FfiConverterOptionalOsGymWorkspacePoolStatus) LowerExternal(value *cyclops_sdk_schema.OsGymWorkspacePoolStatus) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[*cyclops_sdk_schema.OsGymWorkspacePoolStatus](c, value))
}

func (_ FfiConverterOptionalOsGymWorkspacePoolStatus) Write(writer io.Writer, value *cyclops_sdk_schema.OsGymWorkspacePoolStatus) {
	if value == nil {
		writeInt8(writer, 0)
	} else {
		writeInt8(writer, 1)
		cyclops_sdk_schema.FfiConverterOsGymWorkspacePoolStatusINSTANCE.Write(writer, *value)
	}
}

type FfiDestroyerOptionalOsGymWorkspacePoolStatus struct{}

func (_ FfiDestroyerOptionalOsGymWorkspacePoolStatus) Destroy(value *cyclops_sdk_schema.OsGymWorkspacePoolStatus) {
	if value != nil {
		cyclops_sdk_schema.FfiDestroyerOsGymWorkspacePoolStatus{}.Destroy(*value)
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

type FfiConverterSequenceClaim struct{}

var FfiConverterSequenceClaimINSTANCE = FfiConverterSequenceClaim{}

func (c FfiConverterSequenceClaim) Lift(rb RustBufferI) []Claim {
	return LiftFromRustBuffer[[]Claim](c, rb)
}

func (c FfiConverterSequenceClaim) Read(reader io.Reader) []Claim {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]Claim, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterClaimINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceClaim) Lower(value []Claim) C.RustBuffer {
	return LowerIntoRustBuffer[[]Claim](c, value)
}

func (c FfiConverterSequenceClaim) LowerExternal(value []Claim) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]Claim](c, value))
}

func (c FfiConverterSequenceClaim) Write(writer io.Writer, value []Claim) {
	if len(value) > math.MaxInt32 {
		panic("[]Claim is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterClaimINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceClaim struct{}

func (FfiDestroyerSequenceClaim) Destroy(sequence []Claim) {
	for _, value := range sequence {
		FfiDestroyerClaim{}.Destroy(value)
	}
}

type FfiConverterSequenceHttpHeader struct{}

var FfiConverterSequenceHttpHeaderINSTANCE = FfiConverterSequenceHttpHeader{}

func (c FfiConverterSequenceHttpHeader) Lift(rb RustBufferI) []HttpHeader {
	return LiftFromRustBuffer[[]HttpHeader](c, rb)
}

func (c FfiConverterSequenceHttpHeader) Read(reader io.Reader) []HttpHeader {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]HttpHeader, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterHttpHeaderINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequenceHttpHeader) Lower(value []HttpHeader) C.RustBuffer {
	return LowerIntoRustBuffer[[]HttpHeader](c, value)
}

func (c FfiConverterSequenceHttpHeader) LowerExternal(value []HttpHeader) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]HttpHeader](c, value))
}

func (c FfiConverterSequenceHttpHeader) Write(writer io.Writer, value []HttpHeader) {
	if len(value) > math.MaxInt32 {
		panic("[]HttpHeader is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterHttpHeaderINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequenceHttpHeader struct{}

func (FfiDestroyerSequenceHttpHeader) Destroy(sequence []HttpHeader) {
	for _, value := range sequence {
		FfiDestroyerHttpHeader{}.Destroy(value)
	}
}

type FfiConverterSequencePool struct{}

var FfiConverterSequencePoolINSTANCE = FfiConverterSequencePool{}

func (c FfiConverterSequencePool) Lift(rb RustBufferI) []Pool {
	return LiftFromRustBuffer[[]Pool](c, rb)
}

func (c FfiConverterSequencePool) Read(reader io.Reader) []Pool {
	length := readInt32(reader)
	if length == 0 {
		return nil
	}
	result := make([]Pool, 0, length)
	for i := int32(0); i < length; i++ {
		result = append(result, FfiConverterPoolINSTANCE.Read(reader))
	}
	return result
}

func (c FfiConverterSequencePool) Lower(value []Pool) C.RustBuffer {
	return LowerIntoRustBuffer[[]Pool](c, value)
}

func (c FfiConverterSequencePool) LowerExternal(value []Pool) ExternalCRustBuffer {
	return RustBufferFromC(LowerIntoRustBuffer[[]Pool](c, value))
}

func (c FfiConverterSequencePool) Write(writer io.Writer, value []Pool) {
	if len(value) > math.MaxInt32 {
		panic("[]Pool is too large to fit into Int32")
	}

	writeInt32(writer, int32(len(value)))
	for _, item := range value {
		FfiConverterPoolINSTANCE.Write(writer, item)
	}
}

type FfiDestroyerSequencePool struct{}

func (FfiDestroyerSequencePool) Destroy(sequence []Pool) {
	for _, value := range sequence {
		FfiDestroyerPool{}.Destroy(value)
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

const (
	uniffiRustFuturePollReady      int8 = 0
	uniffiRustFuturePollMaybeReady int8 = 1
)

type rustFuturePollFunc func(C.uint64_t, C.UniffiRustFutureContinuationCallback, C.uint64_t)
type rustFutureCompleteFunc[T any] func(C.uint64_t, *C.RustCallStatus) T
type rustFutureFreeFunc func(C.uint64_t)

//export cyclops_sdk_uniffiFutureContinuationCallback
func cyclops_sdk_uniffiFutureContinuationCallback(data C.uint64_t, pollResult C.int8_t) {
	h := cgo.Handle(uintptr(data))
	waiter := h.Value().(chan int8)
	waiter <- int8(pollResult)
}

func uniffiRustCallAsync[E any, T any, F any](
	errConverter BufReader[E],
	completeFunc rustFutureCompleteFunc[F],
	liftFunc func(F) T,
	rustFuture C.uint64_t,
	pollFunc rustFuturePollFunc,
	freeFunc rustFutureFreeFunc,
) (T, E) {
	defer freeFunc(rustFuture)

	pollResult := int8(-1)
	waiter := make(chan int8, 1)

	chanHandle := cgo.NewHandle(waiter)
	defer chanHandle.Delete()

	for pollResult != uniffiRustFuturePollReady {
		pollFunc(
			rustFuture,
			(C.UniffiRustFutureContinuationCallback)(C.cyclops_sdk_uniffiFutureContinuationCallback),
			C.uint64_t(chanHandle),
		)
		pollResult = <-waiter
	}

	var goValue T
	ffiValue, err := rustCallWithError(errConverter, func(status *C.RustCallStatus) F {
		return completeFunc(rustFuture, status)
	})
	if value := reflect.ValueOf(err); value.IsValid() && !value.IsZero() {
		return goValue, err
	}
	return liftFunc(ffiValue), err
}

//export cyclops_sdk_uniffiFreeGorutine
func cyclops_sdk_uniffiFreeGorutine(data C.uint64_t) {
	handle := cgo.Handle(uintptr(data))
	defer handle.Delete()

	guard := handle.Value().(chan struct{})
	guard <- struct{}{}
}
