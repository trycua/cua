package usage

import (
	"context"
	"errors"
	"fmt"
	"io"
	"sync"
	"time"

	awsconfig "github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/smithy-go"
)

type s3QueryObjectStore struct {
	once      sync.Once
	client    *s3.Client
	presigner *s3.PresignClient
	err       error
}

func NewS3QueryObjectStore() QueryObjectStore {
	return &s3QueryObjectStore{}
}

func (store *s3QueryObjectStore) load(ctx context.Context) error {
	store.once.Do(func() {
		configuration, err := awsconfig.LoadDefaultConfig(ctx)
		if err != nil {
			store.err = err
			return
		}
		store.client = s3.NewFromConfig(configuration)
		store.presigner = s3.NewPresignClient(store.client)
	})
	return store.err
}

func (store *s3QueryObjectStore) PresignPut(ctx context.Context, bucket, key, contentType string, expires time.Duration) (string, error) {
	if err := store.load(ctx); err != nil {
		return "", err
	}
	result, err := store.presigner.PresignPutObject(ctx, &s3.PutObjectInput{
		Bucket:      &bucket,
		Key:         &key,
		ContentType: &contentType,
	}, func(options *s3.PresignOptions) {
		options.Expires = expires
	})
	if err != nil {
		return "", err
	}
	return result.URL, nil
}

func (store *s3QueryObjectStore) Get(ctx context.Context, bucket, key string, maxBytes int64) ([]byte, error) {
	if err := store.load(ctx); err != nil {
		return nil, err
	}
	result, err := store.client.GetObject(ctx, &s3.GetObjectInput{Bucket: &bucket, Key: &key})
	if err != nil {
		var apiError smithy.APIError
		if errors.As(err, &apiError) && (apiError.ErrorCode() == "NoSuchKey" || apiError.ErrorCode() == "NotFound") {
			return nil, errors.Join(ErrQueryObjectNotFound, newSanitizedError("query object not found", err))
		}
		return nil, err
	}
	defer result.Body.Close()
	body, err := io.ReadAll(io.LimitReader(result.Body, maxBytes+1))
	if err != nil {
		return nil, err
	}
	if int64(len(body)) > maxBytes {
		return nil, fmt.Errorf("query object exceeds limit")
	}
	return body, nil
}

func (store *s3QueryObjectStore) Delete(ctx context.Context, bucket, key string) error {
	if err := store.load(ctx); err != nil {
		return err
	}
	_, err := store.client.DeleteObject(ctx, &s3.DeleteObjectInput{Bucket: &bucket, Key: &key})
	return err
}
