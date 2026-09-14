# Product Requirements Document

## Working Title

**Intelligence Workbench**

*A living intelligence system that helps analysts build better mental models of complex, evolving situations.*

---

# 1. Vision

The product exists to improve human judgment, not replace it.

Its purpose is to help analysts understand complex world events by transforming fragmented information into coherent, evolving models of reality. Rather than acting as an oracle that produces answers or predictions, it acts as an intelligence partner that synthesizes evidence, identifies the most consequential uncertainties, and presents competing interpretations in a way that enables better human reasoning.

Forecasting is the initial application because it provides an objective benchmark for measuring whether better understanding leads to better decisions. The underlying product is intended to support any activity that requires high-quality judgment under uncertainty.

---

# 2. Product Philosophy

## Human judgment remains the final authority.

The system should never attempt to replace the user's reasoning or encourage passive acceptance of AI-generated conclusions.

Its responsibility ends at producing the highest-quality understanding possible.

The user's responsibility begins with making the judgment.

---

## Understanding is more valuable than prediction.

The primary output is not a probability estimate.

The primary output is a better mental model.

The user should consistently leave the system thinking:

> "I understand this situation differently than I did before."

---

## Organization beats accumulation.

Finding more information is not sufficient.

The product must organize information into explanatory structures.

A report containing fewer facts but revealing the underlying dynamics is more valuable than exhaustive information without structure.

---

## Dissent is evidence.

Consensus should be presented clearly.

Strong opposing viewpoints should always be actively sought.

The goal is not balance for its own sake, but robust understanding.

---

## Knowledge compounds.

Each research effort should strengthen the system's long-term understanding.

Knowledge should accumulate into an evolving representation of the world rather than disappearing after individual reports.

---

# 3. Target User

The primary user is an intelligence analyst working on complex, evolving geopolitical, technological, economic, scientific, or strategic questions.

Characteristics of the user:

* Comfortable reading long-form analytical reports.
* Values nuance over simplicity.
* Makes decisions under uncertainty.
* Prefers evidence over opinions.
* Wants assistance building models rather than outsourcing judgment.
* Frequently revisits topics over weeks or months.

Forecasting practitioners represent the initial beachhead because they naturally fit this profile.

---

# 4. Problem Statement

Today's AI assistants optimize for answering questions.

Intelligence analysts require something fundamentally different.

They need systems capable of:

* synthesizing large amounts of evidence,
* maintaining long-term context,
* identifying causal structure,
* surfacing disagreements,
* exposing hidden assumptions,
* revealing the few variables that dominate uncertainty.

Current tools either generate isolated answers or overwhelm users with information.

Neither improves understanding.

---

# 5. Product Goals

The system should:

* Produce intelligence briefings that materially improve the user's understanding.
* Help users identify the cruxes driving uncertainty.
* Continuously update evolving dossiers as new information becomes available.
* Preserve accumulated knowledge between research sessions.
* Encourage active reasoning without replacing judgment.

---

# 6. Non-Goals

The product is not intended to:

* Automatically forecast outcomes.
* Recommend probabilities by default.
* Replace analysts.
* Optimize for report brevity.
* Maximize automation at the expense of understanding.
* Function as a generic chatbot.

---

# 7. Core Workflow

## Step 1

The user submits a forecasting question or analytical topic.

Example:

"Will Country X conduct a military intervention before the end of 2027?"

---

## Step 2

The system performs comprehensive autonomous research.

This includes:

* recent developments,
* historical background,
* academic literature,
* expert commentary,
* official statements,
* statistical evidence,
* historical analogies,
* relevant base rates,
* competing hypotheses,
* dissenting perspectives,
* important uncertainties.

Research should prioritize reliability over speed.

---

## Step 3

The system constructs or updates a living dossier.

The dossier becomes the canonical representation of everything currently known about the topic.

New evidence updates the dossier rather than generating disconnected reports.

---

## Step 4

The system generates an analytical briefing.

The report should explain:

* what happened,
* why it matters,
* what mechanisms are operating,
* where experts agree,
* where experts disagree,
* historical precedents,
* key causal relationships,
* dominant uncertainties,
* unresolved questions,
* evidence quality,
* assumptions requiring validation.

Importantly, it should answer:

> "What are the few things that explain most of this situation?"

---

## Step 5

The user studies the report.

No prediction is presented.

The emphasis is understanding.

---

## Step 6

The user engages in discussion.

Conversation serves to:

* clarify evidence,
* challenge interpretations,
* request additional research,
* explore counterfactuals,
* investigate disagreements.

Discussion refines understanding rather than replacing the report.

---

## Step 7

The user independently reaches a judgment.

The system supports.

The user decides.

---

# 8. Primary Artifact

The central artifact is a **living intelligence dossier**.

A dossier contains:

* evolving narrative
* timeline
* entities
* events
* relationships
* evidence
* sources
* causal hypotheses
* competing explanations
* unresolved questions
* historical analogies
* forecasts connected to the topic

The dossier persists indefinitely.

Every future research session updates it.

---

# 9. Knowledge Representation

Internally, the system should maintain a structured knowledge graph.

The graph represents:

* people,
* organizations,
* countries,
* technologies,
* events,
* policies,
* claims,
* evidence,
* causal links,
* temporal relationships.

Reports are generated from this evolving representation rather than assembled independently each time.

---

# 10. Report Design Principles

Reports should maximize insight rather than completeness.

Every report should contain:

## Executive Overview

A concise orientation to the problem.

---

## Situation Summary

Current state of affairs.

---

## Historical Context

Relevant precedents.

Long-term trends.

Base rates.

---

## Causal Model

The mechanisms driving the situation.

Not merely chronology.

---

## Competing Hypotheses

Alternative explanations.

Supporting evidence.

Contradicting evidence.

---

## Consensus and Dissent

Areas of agreement.

Areas of disagreement.

Minority viewpoints worth considering.

---

## Crux Analysis

The two or three uncertainties that dominate all others.

These deserve disproportionate attention.

---

## Counterfactual Analysis

"What would have happened if..."

Alternative paths.

Missed turning points.

---

## Evidence Assessment

Confidence in available evidence.

Known blind spots.

Potential biases.

---

## Open Questions

What remains unknown.

What future information would matter most.

---

## Source Appendix

Complete citations.

Primary sources where possible.

---

# 11. Conversation Principles

Chat should assume the report has already been read.

Conversation focuses on:

* clarification,
* exploration,
* deeper analysis,
* alternative perspectives,
* updating understanding.

Conversation should not repeat the report.

---

# 12. Personalization

Over time the system should learn:

* recurring interests,
* preferred depth,
* preferred report structure,
* previous analytical work,
* historical forecasts,
* personal knowledge graph,
* recurring concepts,
* prior assumptions.

The objective is to evolve alongside the analyst.

---

# 13. Forecasting Integration

Forecasting is optional.

The system should never display its own probability estimate unless explicitly requested.

Instead it prepares the user to make an informed forecast.

Forecasts can later be attached to dossiers to track how understanding evolves over time.

---

# 14. Design Principles

The product should consistently favor:

Understanding over answers.

Models over summaries.

Structure over accumulation.

Evidence over opinion.

Judgment over automation.

Persistence over isolated sessions.

Dissent over false consensus.

Insight over brevity.

---

# 15. Success Criteria

The product succeeds if users:

* develop better mental models,
* identify important variables earlier,
* notice previously overlooked evidence,
* understand opposing viewpoints,
* produce better calibrated forecasts,
* return repeatedly because it changes how they think.

The defining success criterion is not faster research.

It is a measurable improvement in the quality of human reasoning.

---

# 16. Guiding Principle

The system should behave like an exceptional intelligence analyst preparing a briefing for another exceptional intelligence analyst.

Its goal is never to think instead of the user.

Its goal is to ensure the user has the best possible foundation for thinking.

