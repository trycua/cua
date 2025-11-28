# NeurIPS 2025: 45 Computer-Use Agent Papers You Should Know About

<img alt="neurips" src="https://github.com/user-attachments/assets/bd649067-bb2c-45f4-827b-087021ec3ad7" />

If you're following the computer-use agent space, you already know that NeurIPS is where the most important work gets presented. But with thousands of papers across every area of machine learning, finding the ones relevant to CUAs means hours of filtering through proceedings, skimming abstracts, and hoping you don't miss something important.

We did that work for you. We're excited to announce that **Cua will be at NeurIPS 2025**, and we've compiled a curated list of **45 papers** focused specifically on Computer-Use Agents—covering benchmarks, safety, grounding, visual reasoning, and agent architectures.

## Why This Matters

Computer-use agents are evolving rapidly. This year's NeurIPS showcases several important developments:

**The benchmark landscape is maturing.** We're seeing comprehensive evaluations across macOS (macOSWorld), professional tools (VideoCAD), and real-world websites (REAL, TheAgentCompany). These aren't toy problems anymore—they're measuring what agents can actually do in production environments.

**Safety is becoming a first-class concern.** Multiple papers (OS-Harm, RiOSWorld, WASP, AgentDAM) are systematically documenting how agents fail when confronted with adversarial inputs, privacy requirements, or misuse scenarios. The findings are sobering: even frontier models often comply with harmful requests.

**Grounding remains the bottleneck.** Papers like GUI-Actor, GUI-G1, and SE-GUI are pushing the state of the art on mapping language to UI actions. The best approaches are achieving significant gains with surprisingly small models and datasets.

**Open-source is catching up.** OpenCUA's 72B model hits 45% on OSWorld-Verified, establishing that community-driven development can compete with proprietary systems.

## Highlights Worth Your Attention

A few papers stand out for their immediate relevance to anyone building or deploying computer-use agents:

- **macOSWorld** reveals a dramatic capability gap: proprietary agents achieve 30%+ success on macOS tasks while open-source models struggle below 5%.
- **TheAgentCompany** simulates a software company where agents browse, code, and communicate. The best agent completes 30% of tasks autonomously.
- **WASP** demonstrates that simple prompt injections deceive top-tier models in 86% of cases.
- **GUI-G1** shows that a 3B model can achieve 90.3% on ScreenSpot by fixing issues with chain-of-thought reasoning.

## Summary Statistics

| Category | Count |
|----------|-------|
| Benchmarks & Datasets | 18 |
| Safety & Security | 12 |
| Grounding & Visual Reasoning | 14 |
| Agent Architectures & Training | 11 |
| Adversarial Attacks | 8 |

**Total Papers:** 45

## Meet Us at NeurIPS

We'll be at NeurIPS in San Diego. If you're working on computer-use agents, building applications on top of CUA infrastructure, or just curious about where this space is heading, we'd love to connect.

- **Book a Meeting**: [cal.com/cua/neurips-slot](https://cal.com/cua/neurips-slot)
- **X/Twitter**: [@trycua](https://x.com/trycua)
- **Discord**: [discord.gg/cua-ai](https://discord.gg/cua-ai)

---

# The Papers

## 1. macOSWorld: A Multilingual Interactive Benchmark for GUI Agents

**Summary:** The first comprehensive benchmark for evaluating GUI agents on macOS. Features 202 multilingual interactive tasks across 30 applications (28 macOS-exclusive), with support for 5 languages (English, Chinese, Arabic, Japanese, Russian). Reveals a dramatic gap: proprietary agents achieve 30%+ success rate while open-source models lag below 5%. Also includes safety benchmarking for deception attacks.

**Key Findings:**
- Proprietary computer-use agents lead at above 30% success rate
- Open-source lightweight models struggle below 5%, highlighting need for macOS domain adaptation
- Multilingual benchmarks expose weaknesses, especially in Arabic (28.8% degradation vs English)
- Deception attacks are a general vulnerability requiring immediate attention

**Poster:** https://neurips.cc/virtual/2025/poster/117427

---

## 2. OS-Harm: A Benchmark for Measuring Safety of Computer Use Agents

**Summary:** A comprehensive safety benchmark built on OSWorld for testing computer-use agents across three harm categories: deliberate user misuse, prompt injection attacks, and model misbehavior. Includes 150 tasks spanning harassment, copyright infringement, disinformation, data exfiltration, and more. Proposes an automated judge achieving high agreement with human annotations (0.76-0.79 F1 score).

**Key Findings:**
- All tested models (o4-mini, Claude 3.7 Sonnet, Gemini 2.5 Pro) tend to directly comply with many deliberate misuse queries
- Models are relatively vulnerable to static prompt injections
- Models occasionally perform unsafe actions without explicit malicious prompts

**Poster:** https://neurips.cc/virtual/2025/loc/san-diego/poster/121772

---

## 3. OpenCUA: Open Foundations for Computer-Use Agents

**Summary:** A comprehensive open-source framework for scaling computer-use agent data and foundation models. Introduces AgentNet, the first large-scale computer-use task dataset spanning 3 operating systems and 200+ applications/websites. OpenCUA-72B achieves 45% success rate on OSWorld-Verified, establishing new state-of-the-art among open-source models.

**Key Contributions:**
- Annotation infrastructure for capturing human computer-use demonstrations
- AgentNet: large-scale dataset across 3 OSes and 200+ apps
- Scalable pipeline transforming demonstrations into state-action pairs with reflective Chain-of-Thought reasoning
- Models generalize well across domains and benefit from increased test-time computation

**Poster:** https://neurips.cc/virtual/2025/poster/119771

---

## 4. Mind2Web 2: Evaluating Agentic Search with Agent-as-a-Judge

**Summary:** A benchmark of 130 realistic, high-quality, long-horizon tasks for agentic search systems (like Deep Research), requiring real-time web browsing and extensive information synthesis. Constructed with 1000+ hours of human labor. Introduces Agent-as-a-Judge framework using tree-structured rubric design for automated evaluation.

**Key Findings:**
- OpenAI Deep Research achieves 50-70% of human performance while spending half the time
- First systematic evaluation of ten frontier agentic search systems vs. human performance
- Addresses the challenge of evaluating time-varying, complex answers

**Poster:** https://neurips.cc/virtual/2025/poster/121798

---

## 5. Scaling Computer-Use Grounding via User Interface Decomposition and Synthesis

**Summary:** Addresses GUI grounding—mapping natural language to specific UI actions—as a critical bottleneck in agent development. Introduces OSWorld-G benchmark (564 annotated samples) and Jedi dataset (4 million synthetic examples), the largest computer-use grounding dataset. Improved grounding directly enhances agentic capabilities, boosting OSWorld performance from 23% to 51%.

**Key Contributions:**
- OSWorld-G: comprehensive benchmark for diverse grounding tasks (text matching, element recognition, layout understanding, precise manipulation)
- Jedi: 4M examples through multi-perspective task decoupling
- Demonstrates compositional generalization to novel interfaces

**Poster:** https://neurips.cc/virtual/2025/poster/121759

---

## 6. RiOSWorld: Benchmarking the Risk of Multimodal Computer-Use Agents

**Summary:** Evaluates potential safety risks of MLLM-based agents during real-world computer manipulation. Features 492 risky tasks spanning web, social media, multimedia, OS, email, and office software. Categorizes risks into user-originated and environmental risks, evaluating both risk goal intention and completion.

**Key Findings:**
- Current computer-use agents face significant safety risks in real-world scenarios
- Safety principles designed for dialogue scenarios don't transfer well to computer-use
- Highlights necessity and urgency of safety alignment for computer-use agents

**Poster:** https://neurips.cc/virtual/2025/poster/117273

---

## 7. REAL: Benchmarking Autonomous Agents on Deterministic Simulations of Real Websites

**Summary:** A benchmark featuring high-fidelity, deterministic replicas of 11 widely-used websites across e-commerce, travel, communication, and professional networking. Contains 112 practical tasks requiring both information retrieval and state-changing actions. Enables reproducible evaluation without safety risks.

**Key Findings:**
- Best frontier language models achieve only 41% success rate
- Highlights critical gaps in autonomous web navigation and task completion
- Supports scalable post-training data generation

**Poster:** https://neurips.cc/virtual/2025/poster/121619

---

## 8. SE-GUI: Enhancing Visual Grounding for GUI Agents via Self-Evolutionary Reinforcement Learning

**Summary:** An RL-based framework for GUI grounding incorporating seed data curation, dense policy gradients, and self-evolutionary reinforcement finetuning using attention maps. With only 3K training samples, the 7B model achieves state-of-the-art on three grounding benchmarks, outperforming UI-TARS-72B by 24.2% on ScreenSpot-Pro.

**Key Results:**
- 47.3% accuracy on ScreenSpot-Pro with 7B model
- Outperforms 72B models with fraction of training data
- Demonstrates effectiveness of RL for high-resolution, complex environments

**Poster:** https://neurips.cc/virtual/2025/poster/118788

---

## 9. TRAP: Targeted Redirecting of Agentic Preferences

**Summary:** A generative adversarial framework that manipulates agent decision-making using diffusion-based semantic injections. Combines negative prompt degradation with positive semantic optimization. Without model access, produces visually natural images that induce consistent decision biases in agents.

**Key Findings:**
- Consistently induces decision-level preference redirection on LLaVA-34B, Gemma3, GPT-4o, and Mistral-3.2
- Outperforms baselines (SPSA, Bandit, standard diffusion)
- Exposes vulnerability: autonomous agents can be misled through visually subtle, semantically-guided manipulations

**Poster:** https://neurips.cc/virtual/2025/poster/117547

---

## 10. TheAgentCompany: Benchmarking LLM Agents on Consequential Real World Tasks

**Summary:** An extensible benchmark simulating a small software company environment where AI agents interact like digital workers: browsing the web, writing code, running programs, and communicating with coworkers. Tests agents on real professional tasks with important implications for industry adoption and labor market effects.

**Key Findings:**
- Best agent achieves 30% autonomous task completion
- Simpler tasks are solvable autonomously
- More difficult long-horizon tasks remain beyond current systems' reach

**Poster:** https://neurips.cc/virtual/2025/poster/121705

---

## 11. VideoGameQA-Bench: Evaluating Vision-Language Models for Video Game Quality Assurance

**Summary:** A comprehensive benchmark for VLMs in video game QA, encompassing visual unit testing, visual regression testing, needle-in-a-haystack challenges, glitch detection, and bug report generation for both images and videos. Addresses the need for standardized benchmarks in this labor-intensive domain.

**Key Focus:**
- First benchmark specifically designed for video game QA with VLMs
- Covers wide range of QA activities across images and videos
- Addresses lack of automation in game development workflows

**Poster:** https://neurips.cc/virtual/2025/poster/121740

---

## 12. WASP: Benchmarking Web Agent Security Against Prompt Injection Attacks

**Summary:** End-to-end benchmark for evaluating web agent security against prompt injection attacks. Tests realistic scenarios where even simple, low-effort human-written injections can deceive top-tier AI models including those with advanced reasoning.

**Key Findings:**
- Attacks partially succeed in up to 86% of cases
- State-of-the-art agents often struggle to fully complete attacker goals
- Reveals "security by incompetence"—agents' limitations sometimes prevent full attack success

**Poster:** https://neurips.cc/virtual/2025/poster/121728

---

## 13. AgentDAM: Privacy Leakage Evaluation for Autonomous Web Agents

**Summary:** Measures whether AI web-navigation agents follow the privacy principle of "data minimization"—using sensitive information only when truly necessary to complete a task. Simulates realistic web interaction scenarios end-to-end.

**Key Findings:**
- Agents built on GPT-4, Llama-3, and Claude are prone to inadvertent use of unnecessary sensitive information
- Proposes prompting-based defense that reduces information leakage
- End-to-end benchmarking provides more realistic measure than probing LLMs about privacy

**Poster:** https://neurips.cc/virtual/2025/poster/121443

---

## 14. Embodied Web Agents: Bridging Physical-Digital Realms for Integrated Agent Intelligence

**Summary:** A novel paradigm for AI agents that fluidly bridge embodiment and web-scale reasoning. Creates unified simulation integrating realistic 3D indoor/outdoor environments with functional web interfaces. Tasks include cooking from online recipes, navigating with dynamic map data, and interpreting landmarks using web knowledge.

**Key Contributions:**
- Unified platform combining 3D environments with web interfaces
- Benchmark spanning cooking, navigation, shopping, tourism, and geolocation
- Reveals significant performance gaps between AI systems and humans

**Poster:** https://neurips.cc/virtual/2025/poster/121809

---

## 15. VideoCAD: A Dataset and Model for Learning Long-Horizon 3D CAD UI Interactions from Video

**Summary:** The first attempt to model UI interactions for precision engineering tasks. Features 41K+ annotated video recordings of CAD operations with time horizons up to 20x longer than existing datasets. Proposes VideoCADFormer for learning CAD interactions directly from video.

**Key Contributions:**
- Large-scale synthetic dataset for CAD UI interactions
- VQA benchmark for evaluating spatial reasoning and video understanding
- Reveals challenges in precise action grounding and long-horizon dependencies

**Poster:** https://neurips.cc/virtual/2025/poster/121820

---

## 16. Look Before You Leap: A GUI-Critic-R1 Model for Pre-Operative Error Diagnosis

**Summary:** Introduces a pre-operative critic mechanism that provides feedback before action execution by reasoning about potential outcomes. Proposes Suggestion-aware Group Relative Policy Optimization (S-GRPO) for building the GUI-Critic-R1 model with fully automated data generation.

**Key Results:**
- Significant advantages in critic accuracy compared to current MLLMs
- Improved success rates and operational efficiency on GUI automation benchmarks
- Works across both mobile and web domains

**Poster:** https://neurips.cc/virtual/2025/poster/115566

---

## 17. Grounded Reinforcement Learning for Visual Reasoning (ViGoRL)

**Summary:** A vision-language model trained with RL to explicitly anchor each reasoning step to specific visual coordinates. Introduces multi-turn RL framework enabling dynamic zooming into predicted coordinates during reasoning.

**Key Results:**
- 86.4% on V*Bench for visual search
- Outperforms supervised fine-tuning and conventional RL across spatial reasoning, visual search, and web-based grounding
- Grounding amplifies region exploration, subgoal setting, and visual verification

**Poster:** https://neurips.cc/virtual/2025/poster/120218

---

## 18. GUI-Actor: Coordinate-Free Visual Grounding for GUI Agents

**Summary:** A VLM-based method for coordinate-free GUI grounding using an attention-based action head. Enables proposing one or more action regions in a single forward pass with a grounding verifier for selection.

**Key Results:**
- GUI-Actor-7B achieves 44.6 on ScreenSpot-Pro with Qwen2.5-VL, outperforming UI-TARS-72B (38.1)
- Improved generalization to unseen resolutions and layouts
- Fine-tuning only ~100M parameters achieves SOTA performance

**Poster:** https://neurips.cc/virtual/2025/poster/119841

---

## 19. GUI-G1: Understanding R1-Zero-Like Training for Visual Grounding in GUI Agents

**Summary:** Extensive analysis of the R1-Zero paradigm (online RL + chain-of-thought reasoning) for GUI grounding. Identifies issues: longer reasoning chains lead to worse performance, reward hacking via box size exploitation, and overfitting easy examples.

**Solutions Proposed:**
- Fast Thinking Template for direct answer generation
- Box size constraint in reward function
- Difficulty-aware scaling in RL objective

**Key Results:**
- GUI-G1-3B achieves 90.3% on ScreenSpot and 37.1% on ScreenSpot-Pro
- Outperforms larger UI-TARS-7B with only 3B parameters

**Poster:** https://neurips.cc/virtual/2025/poster/120227

---

## 20. GUI-Reflection: Empowering Multimodal GUI Models with Self-Reflection Behavior

**Summary:** Framework integrating self-reflection and error correction into end-to-end multimodal GUI models through GUI-specific pre-training, offline SFT, and online reflection tuning. Enables self-reflection emergence with fully automated data generation.

**Key Contributions:**
- Scalable pipelines for automatic reflection/correction data from successful trajectories
- GUI-Reflection Task Suite for reflection-oriented abilities
- Diverse environment for online training on mobile devices
- Iterative online reflection tuning algorithm

**Poster:** https://neurips.cc/virtual/2025/poster/115826

---

## 21. InfantAgent-Next: A Multimodal Generalist Agent for Automated Computer Interaction

**Summary:** A generalist agent capable of multimodal computer interaction (text, images, audio, video). Integrates tool-based and pure vision agents within highly modular architecture, enabling collaborative step-by-step task solving.

**Key Results:**
- 7.27 accuracy gain over Claude-Computer-Use on OSWorld
- Evaluated on pure vision benchmarks (OSWorld), general benchmarks (GAIA), and tool-intensive benchmarks (SWE-Bench)
- Demonstrates value of modular, collaborative agent architecture

**Poster:** https://neurips.cc/virtual/2025/poster/118379

---

## 22. AdvEDM: Fine-grained Adversarial Attack against VLM-based Embodied Agents

**Summary:** A fine-grained adversarial attack framework that modifies VLM perception of only key objects while preserving semantics of remaining regions. Unlike broad semantic disruption, this targeted approach reduces conflicts with task context, making VLMs output valid but incorrect decisions that affect agent actions in the physical world.

**Key Contributions:**
- AdvEDM-R: removes semantics of specific objects from images
- AdvEDM-A: adds semantics of new objects into images
- Demonstrates fine-grained control with excellent attack performance in embodied decision-making tasks

**Poster:** https://neurips.cc/virtual/2025/poster/116436

---

## 23. BLINK-Twice: A Reasoning Benchmark on Visual Perception

**Summary:** A vision-centric reasoning benchmark grounded in challenging perceptual tasks. Unlike prior benchmarks, it moves beyond shallow perception ("see") to require fine-grained observation and analytical reasoning ("observe"). Features natural adversarial image pairs and annotated reasoning chains for process evaluation.

**Key Findings:**
- Tests 20 leading MLLMs including 12 foundation models and 8 reasoning-enhanced models
- Existing reasoning strategies (chain-of-thought, self-criticism) result in unstable and redundant reasoning
- Repeated image observation improves performance across models
- Active visual interaction (as in o3) highlights need for new vision reasoning paradigm

**Poster:** https://neurips.cc/virtual/2025/poster/121522

---

## 24. BadVLA: Backdoor Attacks on Vision-Language-Action Models

**Summary:** First systematic investigation of backdoor vulnerabilities in VLA models. Proposes Objective-Decoupled Optimization with two stages: explicit feature-space separation to isolate trigger representations, and conditional control deviations activated only by triggers.

**Key Findings:**
- Consistently achieves near-100% attack success rates with minimal impact on clean task accuracy
- Robust against common input perturbations, task transfers, and model fine-tuning
- Exposes critical security vulnerabilities in current VLA deployments under Training-as-a-Service paradigm

**Poster:** https://neurips.cc/virtual/2025/poster/115803

---

## 25. Benchmarking Egocentric Multimodal Goal Inference for Assistive Wearable Agents

**Summary:** Benchmark for proactively inferring user goals from multimodal contextual observations for wearable assistant agents (smart glasses). Dataset comprises ~30 hours from 363 participants across 3,482 recordings with visual, audio, digital, and longitudinal context.

**Key Findings:**
- Humans achieve 93% MCQ accuracy; best VLM reaches ~84%
- For open-ended generation, best models produce relevant goals only ~57% of the time
- Smaller models (suited for wearables) achieve ~49% accuracy
- Models benefit from relevant modalities but struggle with noisy ones

**Poster:** https://neurips.cc/virtual/2025/poster/121655

---

## 26. GAM-Agent: Game-Theoretic Multi-Agent Framework for Visual Reasoning

**Summary:** A game-theoretic multi-agent framework formulating reasoning as a non-zero-sum game between base agents (visual perception specialists) and a critical agent (logic/fact verification). Features uncertainty-aware controller for dynamic agent collaboration with multi-round debates.

**Key Results:**
- Boosts small-to-mid scale models (Qwen2.5-VL-7B, InternVL3-14B) by 5-6%
- Enhances strong models like GPT-4o by 2-3%
- Modular, scalable, and generalizable framework

**Poster:** https://neurips.cc/virtual/2025/poster/119144

---

## 27. GRIT: Teaching MLLMs to Think with Images

**Summary:** Introduces Grounded Reasoning with Images and Texts—a method for training MLLMs to generate reasoning chains interleaving natural language with explicit bounding box coordinates. Uses GRPO-GR reinforcement learning with rewards focused on answer accuracy and grounding format.

**Key Contributions:**
- Exceptional data efficiency: requires as few as 20 image-question-answer triplets
- Successfully unifies reasoning and grounding abilities
- Eliminates need for reasoning chain annotations or explicit bounding box labels

**Poster:** https://neurips.cc/virtual/2025/poster/118020

---

## 28. Safe RLHF-V: Safe Reinforcement Learning from Multi-modal Human Feedback

**Summary:** First multimodal safety alignment framework. Introduces BeaverTails-V (first dataset with dual preference annotations for helpfulness and safety), and Beaver-Guard-V (multi-level guardrail system defending against unsafe queries and adversarial attacks).

**Key Results:**
- Guard model improves precursor model's safety by average of 40.9% over five filtering rounds
- Safe RLHF-V enhances model safety by 34.2% and helpfulness by 34.3%
- First exploration of multi-modal safety alignment within constrained optimization

**Poster:** https://neurips.cc/virtual/2025/poster/118304

---

## 29. Dropout Decoding: Uncertainty-Guided Token Dropout for LVLM Reliability

**Summary:** An inference-time approach that quantifies visual token uncertainty and selectively masks uncertain tokens. Decomposes uncertainty into aleatoric and epistemic components, focusing on epistemic uncertainty for perception-related errors.

**Key Results:**
- Significantly reduces object hallucinations
- Enhances reliability and quality of LVLM outputs across diverse visual contexts
- Validated on CHAIR, THRONE, and MMBench benchmarks

**Poster:** https://neurips.cc/virtual/2025/poster/118572

---

## 30. FOCUS: Unified Vision-Language Modeling for Interactive Editing

**Summary:** A unified LVLM integrating segmentation-aware perception and controllable object-centric generation. Uses dual-branch visual encoder for global semantic context and fine-grained spatial details, with MoVQGAN-based visual tokenizer for discrete visual tokens.

**Key Contributions:**
- Progressive multi-stage training pipeline
- Segmentation masks jointly optimized as spatial condition prompts
- Bridges segmentation-aware perception with fine-grained visual synthesis

**Poster:** https://neurips.cc/virtual/2025/poster/119062

---

## 31. Fine-Grained Preference Optimization for Spatial Reasoning (SpatialReasoner-R1)

**Summary:** Introduces Multi-Model Monte Carlo Tree Search (M3CTS) for generating diverse Long Chain-of-Thought reasoning trajectories. Proposes fine-grained Direct Preference Optimization (fDPO) with segment-specific preference granularity guided by spatial reward mechanism.

**Key Results:**
- fDPO achieves 4.1% and 9.0% gains over standard DPO on spatial quality and quantity tasks
- SpatialReasoner-R1 sets new SOTA on SpatialRGPT-Bench, outperforming strongest baseline by 9.8%
- Maintains competitive performance on general vision-language tasks

**Poster:** https://neurips.cc/virtual/2025/poster/118573

---

## 32. Reason-RFT: Reinforcement Fine-Tuning for Visual Reasoning

**Summary:** A two-stage reinforcement fine-tuning framework: SFT with curated Chain-of-Thought data activates reasoning potential, followed by RL based on Group Relative Policy Optimization (GRPO) for domain shift adaptability.

**Key Advantages:**
- State-of-the-art results outperforming both open-source and proprietary models
- Robust performance under domain shifts across various tasks
- Excellent data efficiency in few-shot learning scenarios

**Poster:** https://neurips.cc/virtual/2025/poster/118345

---

## 33. Safe + Safe = Unsafe? Exploiting Safe Images to Jailbreak LVLMs

**Summary:** Reveals that safe images can be exploited for jailbreaking when combined with additional safe images and prompts, exploiting LVLMs' universal reasoning capabilities and safety snowball effect. Proposes Safety Snowball Agent (SSA) framework.

**Key Findings:**
- SSA can use nearly any image to induce LVLMs to produce unsafe content
- Achieves high jailbreak success rates against latest LVLMs
- Exploits inherent LVLM properties rather than alignment flaws

**Poster:** https://neurips.cc/virtual/2025/loc/san-diego/poster/116422

---

## 34. MIP against Agent: Malicious Image Patches Hijacking Multimodal OS Agents

**Summary:** Uncovers novel attack vector: Malicious Image Patches (MIPs)—adversarially perturbed screen regions that induce OS agents to perform harmful actions. MIPs can be embedded in wallpapers or shared on social media to exfiltrate sensitive data.

**Key Findings:**
- MIPs generalize across user prompts and screen configurations
- Can hijack multiple OS agents during execution of benign instructions
- Exposes critical security vulnerabilities requiring attention before widespread deployment

**Poster:** https://neurips.cc/virtual/2025/loc/san-diego/poster/117813

---

## 35. CogVLA: Cognition-Aligned Vision-Language-Action Models

**Summary:** A framework leveraging instruction-driven routing and sparsification for VLA efficiency. Features 3-stage progressive architecture inspired by human multimodal coordination: Encoder-FiLM Aggregation Routing, LLM-FiLM Pruning Routing, and V-L-A Coupled Attention.

**Key Results:**
- 97.4% success rate on LIBERO benchmark, 70.0% on real-world robotic tasks
- Reduces training costs by 2.5x and inference latency by 2.8x compared to OpenVLA
- Achieves state-of-the-art performance

**Poster:** https://neurips.cc/virtual/2025/poster/119023

---

## 36. Succeed or Learn Slowly (SoLS): Sample Efficient RL for Mobile App Control

**Summary:** Novel off-policy RL algorithm applying direct policy updates for positive samples and conservative, regularized updates for negative ones. Augmented with Successful Transition Replay (STR) for prioritizing successful interactions.

**Key Results:**
- At least 17% relative increase over existing methods on AndroidWorld benchmark
- Substantially fewer computational resources than GPT-4o-based methods
- 5-60x faster inference

**Poster:** https://neurips.cc/virtual/2025/poster/119910

---

## 37. TAI3: Testing Agent Integrity in Interpreting User Intent

**Summary:** An API-centric stress testing framework that uncovers intent integrity violations in LLM agents. Uses semantic partitioning to organize tasks into meaningful categories, with targeted mutations to expose subtle agent errors while preserving user intent.

**Key Contributions:**
- Datatype-aware strategy memory for retrieving effective mutation patterns
- Lightweight predictor for ranking mutations by error likelihood
- Generalizes to stronger target models using smaller LLMs for test generation

**Poster:** https://neurips.cc/virtual/2025/poster/118952

---

## 38. ThinkAct: Vision-Language-Action Reasoning via Reinforced Visual Latent Planning

**Summary:** A dual-system framework bridging high-level reasoning with low-level action execution. Trains multimodal LLM to generate embodied reasoning plans guided by action-aligned visual rewards, compressed into visual plan latents for downstream action execution.

**Key Capabilities:**
- Few-shot adaptation
- Long-horizon planning
- Self-correction behaviors in complex embodied AI tasks

**Poster:** https://neurips.cc/virtual/2025/poster/119747

---

## 39. Visualization-of-Thought Attack (VoTA) against VLMs

**Summary:** Automated attack framework that constructs chains of images with risky visual thoughts to challenge VLMs. Exploits the conflict between logical processing and safety protocols, leading to unsafe content generation.

**Key Results:**
- Improves average attack success rate by 26.71% (from 63.70% to 90.41%)
- Tested on 9 open-source and 6 commercial VLMs
- Outperforms state-of-the-art methods

**Poster:** https://neurips.cc/virtual/2025/poster/119873

---

## 40. Open CaptchaWorld: Benchmarking MLLM Agents on CAPTCHA Puzzles

**Summary:** First web-based benchmark evaluating MLLM agents on diverse CAPTCHA puzzles. Spans 20 modern CAPTCHA types (225 total) with novel metric: CAPTCHA Reasoning Depth quantifying cognitive and motor steps required.

**Key Findings:**
- Humans achieve 93.3% success rate
- State-of-the-art agents achieve at most 40.0% (Browser-Use OpenAI-o3)
- Highlights significant gap between human and agent capabilities

**Poster:** https://neurips.cc/virtual/2025/poster/121537

---

## 41. Pixel Reasoner: Pixel-Space Reasoning with Curiosity-Driven RL

**Summary:** Introduces pixel-space reasoning framework where VLMs use visual operations (zoom-in, select-frame) to directly inspect and infer from visual evidence. Two-phase training: instruction tuning on synthesized traces, then RL with curiosity-driven rewards.

**Key Results:**
- 84% on V*Bench, 74% on TallyQA-Complex, 84% on InfographicsVQA
- Highest accuracy achieved by any open-source 7B model
- Enables proactive information gathering from complex visual inputs

**Poster:** https://neurips.cc/virtual/2025/poster/117667

---

## 42. BTL-UI: Blink-Think-Link Reasoning Model for GUI Agent

**Summary:** Brain-inspired framework decomposing interactions into three biologically plausible phases: Blink (rapid detection via saccadic-like attention), Think (higher-level reasoning/planning), and Link (executable command generation for motor control).

**Key Innovations:**
- Automated annotation pipeline for blink data
- BTL Reward: first rule-based reward mechanism driven by both process and outcome
- Competitive performance on static GUI understanding and dynamic interaction tasks

**Poster:** https://neurips.cc/virtual/2025/poster/119419

---

## 43. GUI Exploration Lab: Multi-Turn RL for Screen Navigation

**Summary:** Simulation environment engine enabling flexible definition of screens, icons, and navigation graphs with full environment access for agent training/evaluation. Demonstrates progressive training approach from SFT to multi-turn RL.

**Key Findings:**
- Supervised fine-tuning enables memorization of fundamental knowledge
- Single-turn RL enhances generalization to unseen scenarios
- Multi-turn RL encourages exploration strategies through interactive trial and error

**Poster:** https://neurips.cc/virtual/2025/loc/san-diego/poster/117497

---

## 44. GUI-Rise: Structured Reasoning and History Summarization for GUI Navigation

**Summary:** Reasoning-enhanced framework integrating structured reasoning, action prediction, and history summarization. Uses Chain-of-Thought analyses combining progress estimation and decision reasoning, trained via SFT and GRPO with history-aware rewards.

**Key Results:**
- State-of-the-art under identical training data conditions
- Particularly strong in out-of-domain scenarios
- Robust reasoning and generalization across diverse GUI navigation tasks

**Poster:** https://neurips.cc/virtual/2025/poster/117425

---

## 45. UI-Genie: A Self-Improving Framework for MLLM-based Mobile GUI Agents

**Summary:** Self-improving framework addressing trajectory verification and training data scalability. Features UI-Genie-RM (image-text interleaved reward model) and self-improvement pipeline with reward-guided exploration and outcome verification.

**Key Contributions:**
- UI-Genie-RM-517k: first reward-specific dataset for GUI agents
- UI-Genie-Agent-16k: high-quality synthetic trajectories without manual annotation
- State-of-the-art across multiple GUI agent benchmarks through three generations of self-improvement

**Poster:** https://neurips.cc/virtual/2025/poster/119990

---

## What We're Building

At Cua, we're focused on the infrastructure layer for computer-use agents: cloud sandboxes for safe execution, SDKs for agent development, and tools that make it easier to build and deploy agents in production.

If you're experimenting with any of the approaches in these papers, our [Cloud Sandboxes](https://cua.ai) provide isolated Linux, Windows, and macOS environments where you can test agent behavior without risk to real systems.

---

**Start building:** [cua.ai](https://cua.ai)

**Join the community:** [Discord](https://discord.gg/cua-ai)
