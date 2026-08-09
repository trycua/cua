package statequery

import (
	"errors"
	"fmt"
	"strings"

	pg_query "github.com/pganalyze/pg_query_go/v6"
	"google.golang.org/protobuf/reflect/protoreflect"
)

var ErrInvalidQuery = errors.New("invalid state query")

type Limits struct {
	MaxRows     int
	TimeoutMS   int
	MaxSQLBytes int
}

type ValidatedQuery struct {
	SQL       string
	MaxRows   int
	TimeoutMS int
}

func DefaultLimits() Limits {
	return Limits{MaxRows: 1000, TimeoutMS: 5000, MaxSQLBytes: 65536}
}

func Validate(sql string, limits Limits) (ValidatedQuery, error) {
	sql = strings.TrimSpace(sql)
	if sql == "" {
		return ValidatedQuery{}, fmt.Errorf("%w: SQL is empty", ErrInvalidQuery)
	}
	if limits.MaxRows <= 0 || limits.TimeoutMS <= 0 || limits.MaxSQLBytes <= 0 {
		return ValidatedQuery{}, fmt.Errorf("%w: invalid server limits", ErrInvalidQuery)
	}
	if len(sql) > limits.MaxSQLBytes {
		return ValidatedQuery{}, fmt.Errorf("%w: SQL exceeds byte limit", ErrInvalidQuery)
	}

	tree, err := pg_query.Parse(sql)
	if err != nil {
		return ValidatedQuery{}, fmt.Errorf("%w: parse failed", ErrInvalidQuery)
	}
	if len(tree.Stmts) != 1 || tree.Stmts[0].Stmt.GetSelectStmt() == nil {
		return ValidatedQuery{}, fmt.Errorf("%w: exactly one SELECT is required", ErrInvalidQuery)
	}

	ctes := map[string]bool{}
	if err := walkMessage(tree.ProtoReflect(), func(message protoreflect.Message) error {
		if cte, ok := message.Interface().(*pg_query.CommonTableExpr); ok {
			ctes[cte.GetCtename()] = true
		}
		return nil
	}); err != nil {
		return ValidatedQuery{}, err
	}
	if err := walkMessage(tree.ProtoReflect(), func(message protoreflect.Message) error {
		switch value := message.Interface().(type) {
		case *pg_query.InsertStmt, *pg_query.UpdateStmt, *pg_query.DeleteStmt,
			*pg_query.MergeStmt, *pg_query.CopyStmt, *pg_query.VariableSetStmt,
			*pg_query.TransactionStmt, *pg_query.CreateStmt, *pg_query.AlterTableStmt,
			*pg_query.DropStmt, *pg_query.TruncateStmt, *pg_query.CallStmt,
			*pg_query.DoStmt, *pg_query.IntoClause:
			return fmt.Errorf("%w: writable or utility statement", ErrInvalidQuery)
		case *pg_query.RangeVar:
			if value.GetSchemaname() == "" && ctes[value.GetRelname()] {
				return nil
			}
			if value.GetSchemaname() != "k8s_api" || value.GetRelname() != "current_resources" {
				return fmt.Errorf("%w: relation is not allowlisted", ErrInvalidQuery)
			}
		case *pg_query.FuncCall:
			name := functionName(value)
			if !allowedFunction(name) {
				return fmt.Errorf("%w: function %s is not allowlisted", ErrInvalidQuery, name)
			}
		}
		return nil
	}); err != nil {
		return ValidatedQuery{}, err
	}

	inner := strings.TrimSuffix(sql, ";")
	return ValidatedQuery{
		SQL:       fmt.Sprintf("SELECT * FROM (%s) AS cyclops_state_query LIMIT %d", inner, limits.MaxRows+1),
		MaxRows:   limits.MaxRows,
		TimeoutMS: limits.TimeoutMS,
	}, nil
}

func walkMessage(message protoreflect.Message, visit func(protoreflect.Message) error) error {
	if !message.IsValid() {
		return nil
	}
	if err := visit(message); err != nil {
		return err
	}
	var walkErr error
	message.Range(func(field protoreflect.FieldDescriptor, value protoreflect.Value) bool {
		if field.IsList() && field.Kind() == protoreflect.MessageKind {
			list := value.List()
			for index := 0; index < list.Len(); index++ {
				if err := walkMessage(list.Get(index).Message(), visit); err != nil {
					walkErr = err
					return false
				}
			}
			return true
		}
		if field.Kind() == protoreflect.MessageKind {
			walkErr = walkMessage(value.Message(), visit)
			return walkErr == nil
		}
		return true
	})
	return walkErr
}

func functionName(call *pg_query.FuncCall) string {
	parts := make([]string, 0, len(call.GetFuncname()))
	for _, node := range call.GetFuncname() {
		if value := node.GetString_(); value != nil {
			parts = append(parts, strings.ToLower(value.GetSval()))
		}
	}
	return strings.Join(parts, ".")
}

func allowedFunction(name string) bool {
	switch name {
	case "count", "min", "max", "sum", "avg", "coalesce", "lower", "upper",
		"length", "jsonb_typeof", "jsonb_path_exists", "date_trunc":
		return true
	default:
		return false
	}
}
