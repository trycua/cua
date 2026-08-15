package main

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"regexp"
	"strings"

	"sigs.k8s.io/yaml"
)

var (
	servingNames      = []string{"cyclops-cs", "cyclops-cs-backend"}
	projectorImageTag = regexp.MustCompile(`^main-[0-9]+$`)
)

type manifestObject struct {
	APIVersion string `json:"apiVersion"`
	Kind       string `json:"kind"`
	Metadata   struct {
		Name      string `json:"name"`
		Namespace string `json:"namespace"`
	} `json:"metadata"`
	Spec struct {
		Replicas                int `json:"replicas"`
		ProgressDeadlineSeconds int `json:"progressDeadlineSeconds"`
		Strategy                struct {
			Type          string `json:"type"`
			RollingUpdate struct {
				MaxUnavailable int `json:"maxUnavailable"`
				MaxSurge       int `json:"maxSurge"`
			} `json:"rollingUpdate"`
		} `json:"strategy"`
		Selector labelSelector `json:"selector"`
		Template struct {
			Metadata struct {
				Labels map[string]string `json:"labels"`
			} `json:"metadata"`
			Spec struct {
				TopologySpreadConstraints []topologySpreadConstraint `json:"topologySpreadConstraints"`
				Containers                []container                `json:"containers"`
			} `json:"spec"`
		} `json:"template"`
		MinAvailable int `json:"minAvailable"`
	} `json:"spec"`
}

type labelSelector struct {
	MatchLabels map[string]string `json:"matchLabels"`
}

type topologySpreadConstraint struct {
	MaxSkew           int           `json:"maxSkew"`
	TopologyKey       string        `json:"topologyKey"`
	WhenUnsatisfiable string        `json:"whenUnsatisfiable"`
	LabelSelector     labelSelector `json:"labelSelector"`
}

type container struct {
	Name           string `json:"name"`
	Image          string `json:"image"`
	ReadinessProbe probe  `json:"readinessProbe"`
	LivenessProbe  probe  `json:"livenessProbe"`
}

type probe struct {
	HTTPGet struct {
		Path string `json:"path"`
	} `json:"httpGet"`
}

func main() {
	if err := run(os.Args[1:], os.Stdin); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run(args []string, stdin io.Reader) error {
	if len(args) == 0 {
		return verify(stdin)
	}

	mode := "serving"
	path := args[0]
	if args[0] == "projector" {
		mode = "projector"
		if len(args) == 1 {
			return verifyProjector(stdin)
		}
		if len(args) != 2 {
			return fmt.Errorf("usage: manifest-contract [manifest.yaml] | manifest-contract projector [manifest.yaml]")
		}
		path = args[1]
	} else if len(args) != 1 {
		return fmt.Errorf("usage: manifest-contract [manifest.yaml] | manifest-contract projector [manifest.yaml]")
	}

	file, err := os.Open(path)
	if err != nil {
		return err
	}
	defer file.Close()

	if mode == "projector" {
		return verifyProjector(file)
	}
	return verify(file)
}

func verify(reader io.Reader) error {
	objects, err := parseDocuments(reader)
	if err != nil {
		return err
	}

	for _, name := range servingNames {
		deployment, err := findObject(objects, "Deployment", name)
		if err != nil {
			return err
		}
		if err := verifyDeployment(deployment, name); err != nil {
			return err
		}

		pdb, err := findObject(objects, "PodDisruptionBudget", name)
		if err != nil {
			return err
		}
		if err := verifyPDB(pdb, name); err != nil {
			return err
		}
	}

	for _, object := range objects {
		if object.Metadata.Name == "k8s-state-projector" || object.Metadata.Name == "cyclops-cs-k8s-state-projector" {
			return fmt.Errorf("serving manifest must not contain state projector")
		}
	}

	backend, err := findObject(objects, "Deployment", "cyclops-cs-backend")
	if err != nil {
		return err
	}
	return verifyBackendProbes(backend)
}

func parseDocuments(reader io.Reader) ([]manifestObject, error) {
	var documents []manifestObject
	var current strings.Builder
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 64*1024), 10*1024*1024)
	for scanner.Scan() {
		line := scanner.Text()
		if strings.TrimSpace(line) == "---" {
			if err := appendDocument(&documents, current.String()); err != nil {
				return nil, err
			}
			current.Reset()
			continue
		}
		current.WriteString(line)
		current.WriteByte('\n')
	}
	if err := scanner.Err(); err != nil {
		return nil, err
	}
	if err := appendDocument(&documents, current.String()); err != nil {
		return nil, err
	}
	return documents, nil
}

func appendDocument(documents *[]manifestObject, source string) error {
	if strings.TrimSpace(source) == "" {
		return nil
	}

	var object manifestObject
	if err := yaml.Unmarshal([]byte(source), &object); err != nil {
		return fmt.Errorf("parse manifest document: %w", err)
	}
	*documents = append(*documents, object)
	return nil
}

func findObject(objects []manifestObject, kind, name string) (manifestObject, error) {
	var matches []manifestObject
	for _, object := range objects {
		if object.Kind == kind && object.Metadata.Name == name {
			matches = append(matches, object)
		}
	}
	if len(matches) != 1 {
		return manifestObject{}, fmt.Errorf("expected exactly one %s %q, found %d", kind, name, len(matches))
	}
	return matches[0], nil
}

func verifyDeployment(deployment manifestObject, name string) error {
	if deployment.APIVersion != "apps/v1" || deployment.Metadata.Namespace != "cyclops-cs" {
		return fmt.Errorf("Deployment %q has wrong apiVersion or namespace", name)
	}
	if deployment.Spec.Replicas != 2 || deployment.Spec.ProgressDeadlineSeconds != 600 {
		return fmt.Errorf("Deployment %q must set replicas 2 and progressDeadlineSeconds 600", name)
	}
	if deployment.Spec.Strategy.Type != "RollingUpdate" || deployment.Spec.Strategy.RollingUpdate.MaxUnavailable != 0 || deployment.Spec.Strategy.RollingUpdate.MaxSurge != 1 {
		return fmt.Errorf("Deployment %q must use RollingUpdate maxUnavailable 0 and maxSurge 1", name)
	}
	if deployment.Spec.Selector.MatchLabels["app"] != name {
		return fmt.Errorf("Deployment %q selector app label must equal %q", name, name)
	}
	if deployment.Spec.Template.Metadata.Labels["app"] != name {
		return fmt.Errorf("Deployment %q pod template app label must equal %q", name, name)
	}
	for _, constraint := range deployment.Spec.Template.Spec.TopologySpreadConstraints {
		if constraint.MaxSkew == 1 && constraint.TopologyKey == "kubernetes.io/hostname" && constraint.WhenUnsatisfiable == "ScheduleAnyway" && constraint.LabelSelector.MatchLabels["app"] == name {
			return nil
		}
	}
	return fmt.Errorf("Deployment %q must spread pods by app label %q", name, name)
}

func verifyPDB(pdb manifestObject, name string) error {
	if pdb.APIVersion != "policy/v1" || pdb.Metadata.Namespace != "cyclops-cs" {
		return fmt.Errorf("PodDisruptionBudget %q has wrong apiVersion or namespace", name)
	}
	if pdb.Spec.MinAvailable != 1 || pdb.Spec.Selector.MatchLabels["app"] != name {
		return fmt.Errorf("PodDisruptionBudget %q must select app %q with minAvailable 1", name, name)
	}
	return nil
}

func verifyBackendProbes(backend manifestObject) error {
	for _, container := range backend.Spec.Template.Spec.Containers {
		if container.Name == "cyclops-cs-backend" && container.ReadinessProbe.HTTPGet.Path == "/healthz" && container.LivenessProbe.HTTPGet.Path == "/healthz" {
			return nil
		}
	}
	return fmt.Errorf("Deployment %q backend container must use /healthz readiness and liveness probes", backend.Metadata.Name)
}

func verifyProjector(reader io.Reader) error {
	objects, err := parseDocuments(reader)
	if err != nil {
		return err
	}

	projector, err := findObject(objects, "Deployment", "k8s-state-projector")
	if err != nil {
		return err
	}
	if projector.APIVersion != "apps/v1" || projector.Metadata.Namespace != "cyclops-cs" {
		return fmt.Errorf("projector Deployment has wrong apiVersion or namespace")
	}

	for _, container := range projector.Spec.Template.Spec.Containers {
		if container.Name != "projector" {
			continue
		}
		return verifyProjectorImage(container.Image)
	}
	return fmt.Errorf("projector Deployment must contain projector container")
}

func verifyProjectorImage(image string) error {
	const repository = "296062593712.dkr.ecr.us-west-2.amazonaws.com/cyclops-cs-backend"

	imageRepository, tag, found := strings.Cut(image, ":")
	if !found || imageRepository != repository {
		return fmt.Errorf("projector container image repository must equal %q", repository)
	}
	if !projectorImageTag.MatchString(tag) {
		return fmt.Errorf("projector container image tag must match ^main-[0-9]+$")
	}
	return nil
}
