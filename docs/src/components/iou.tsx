'use client';
import React, { useRef, useEffect, useState, useCallback } from 'react';

interface Rectangle {
  left: number;
  top: number;
  width: number;
  height: number;
  fill: string;
  name: string;
}

interface IOUProps {
  title: string;
  description: string;
  rect1: Rectangle;
  rect2: Rectangle;
}

export default function IOU({ title, description, rect1, rect2 }: IOUProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [actualIOU, setActualIOU] = useState<number>(0);

  const getBbox = (rect: Rectangle) => ({
    left: rect.left,
    right: rect.left + rect.width,
    top: rect.top,
    bottom: rect.top + rect.height,
  });

  const calcIntersection = (bbox1: any, bbox2: any): number => {
    const x1 = Math.max(bbox1.left, bbox2.left);
    const x2 = Math.min(bbox1.right, bbox2.right);
    const y1 = Math.max(bbox1.top, bbox2.top);
    const y2 = Math.min(bbox1.bottom, bbox2.bottom);

    // Check if there's actually an overlap
    if (x2 <= x1 || y2 <= y1) {
      return 0;
    }

    const intersection = (x2 - x1) * (y2 - y1);
    return intersection;
  };

  const calcArea = (rect: Rectangle): number => {
    return rect.width * rect.height;
  };

  const drawCanvas = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    // Clear canvas
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    // Calculate IOU
    const bbox1 = getBbox(rect1);
    const bbox2 = getBbox(rect2);
    const intersection = calcIntersection(bbox1, bbox2);
    const union = calcArea(rect1) + calcArea(rect2) - intersection;
    const iou = intersection / union;
    setActualIOU(iou);

    // Draw rectangles
    [rect1, rect2].forEach((rect) => {
      ctx.fillStyle = rect.fill;
      ctx.fillRect(rect.left, rect.top, rect.width, rect.height);

      ctx.strokeStyle = '#000';
      ctx.lineWidth = 2;
      ctx.strokeRect(rect.left, rect.top, rect.width, rect.height);

      ctx.fillStyle = '#000';
      ctx.font = '12px';
      ctx.fillText(rect.name, rect.left + 5, rect.top + 15);
    });
  }, [rect1, rect2]);

  useEffect(() => {
    drawCanvas();
  }, [drawCanvas]);

  return (
    <div className="">
      <h3 className="text-sm font-semibold ">{title}</h3>
      <div className="flex items-start gap-6">
        <div>
          <canvas
            ref={canvasRef}
            width={200}
            height={150}
            className="border bg-white rounded-md"
          />
          <div className="mt-2 text-sm">
            <div className="font-mono mb-2">IOU = {actualIOU.toFixed(3)}</div>
            <span className="">{description}</span>
          </div>
        </div>
      </div>
    </div>
  );
}
