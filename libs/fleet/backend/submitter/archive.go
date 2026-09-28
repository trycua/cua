package submitter

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"time"

	awsconfig "github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
)

// Archive is the S3 representation of one batch: newline-delimited JSON rows
// plus a manifest written last, mirroring the opencost-pipeline export
// contract (manifest presence means the data object is complete).
type Archive struct {
	Data     []byte
	Manifest []byte
	SHA256   string
}

type archiveRow struct {
	SubmissionID     string `json:"submission_id"`
	BatchID          string `json:"batch_id"`
	CapsuleTenant    string `json:"capsule_tenant"`
	HourStart        string `json:"hour_start"`
	Meter            string `json:"meter"`
	Seq              int    `json:"seq"`
	Identifier       string `json:"identifier"`
	StripeCustomerID string `json:"stripe_customer_id,omitempty"`
	Quantity         string `json:"quantity"`
	FactSetSHA256    string `json:"fact_set_sha256"`
}

type archiveManifest struct {
	SchemaVersion   string `json:"schema_version"`
	Dataset         string `json:"dataset"`
	BatchID         string `json:"batch_id"`
	Cluster         string `json:"cluster"`
	CutoverHour     string `json:"cutover_hour"`
	WindowStart     string `json:"window_start"`
	WindowEnd       string `json:"window_end"`
	DryRun          bool   `json:"dry_run"`
	ExporterVersion string `json:"exporter_version"`
	ExportedAt      string `json:"exported_at"`
	DataKey         string `json:"data_key"`
	DataSHA256      string `json:"data_sha256"`
	RowCount        int    `json:"row_count"`
}

// BuildArchive renders rows deterministically (sorted by identifier) so a
// resumed batch produces byte-identical objects.
func BuildArchive(batch Batch, rows []Submission, exporterVersion string, exportedAt time.Time) (Archive, error) {
	if len(rows) == 0 {
		return Archive{}, errors.New("archive requires at least one submission")
	}
	sorted := make([]Submission, len(rows))
	copy(sorted, rows)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].Identifier < sorted[j].Identifier })

	var data bytes.Buffer
	encoder := json.NewEncoder(&data)
	for _, row := range sorted {
		if row.BatchID != batch.BatchID {
			return Archive{}, fmt.Errorf("submission %s belongs to batch %s, not %s", row.Identifier, row.BatchID, batch.BatchID)
		}
		if err := encoder.Encode(archiveRow{
			SubmissionID:     row.SubmissionID.String(),
			BatchID:          row.BatchID.String(),
			CapsuleTenant:    row.Tenant,
			HourStart:        row.HourStart.UTC().Format(time.RFC3339),
			Meter:            row.Meter,
			Seq:              row.Seq,
			Identifier:       row.Identifier,
			StripeCustomerID: row.StripeCustomerID,
			Quantity:         row.Quantity,
			FactSetSHA256:    row.FactSetSHA256,
		}); err != nil {
			return Archive{}, fmt.Errorf("encode archive row %s: %w", row.Identifier, err)
		}
	}
	digest := sha256.Sum256(data.Bytes())
	sha := hex.EncodeToString(digest[:])

	manifest, err := json.Marshal(archiveManifest{
		SchemaVersion:   SchemaVersion,
		Dataset:         ArchiveDataset,
		BatchID:         batch.BatchID.String(),
		Cluster:         batch.ClusterID,
		CutoverHour:     batch.CutoverHour.UTC().Format(time.RFC3339),
		WindowStart:     batch.WindowStart.UTC().Format(time.RFC3339),
		WindowEnd:       batch.WindowEnd.UTC().Format(time.RFC3339),
		DryRun:          batch.DryRun,
		ExporterVersion: exporterVersion,
		ExportedAt:      exportedAt.UTC().Format(time.RFC3339),
		DataKey:         batch.ArchiveKey,
		DataSHA256:      sha,
		RowCount:        len(sorted),
	})
	if err != nil {
		return Archive{}, fmt.Errorf("encode archive manifest: %w", err)
	}
	return Archive{Data: data.Bytes(), Manifest: append(manifest, '\n'), SHA256: sha}, nil
}

// S3Archiver writes archive objects with the ambient AWS credential chain
// (web identity in the CronJob).
type S3Archiver struct {
	client *s3.Client
}

func NewS3Archiver(ctx context.Context) (*S3Archiver, error) {
	configuration, err := awsconfig.LoadDefaultConfig(ctx)
	if err != nil {
		return nil, fmt.Errorf("load AWS configuration: %w", err)
	}
	return &S3Archiver{client: s3.NewFromConfig(configuration)}, nil
}

func (a *S3Archiver) Put(ctx context.Context, bucket, key, contentType string, body []byte) error {
	if _, err := a.client.PutObject(ctx, &s3.PutObjectInput{
		Bucket:      &bucket,
		Key:         &key,
		ContentType: &contentType,
		Body:        bytes.NewReader(body),
	}); err != nil {
		return fmt.Errorf("put s3://%s/%s: %w", bucket, key, err)
	}
	return nil
}
